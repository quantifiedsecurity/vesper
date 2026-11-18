//! Bounds-check hoisting for counted loops (Stage 1 performance wave, lane 3).
//!
//! A native-path Place IR pass that runs only when the optimizer is on,
//! after `ir_opt::optimize` and drop elaboration. It never runs for the
//! verified backend or the evaluator, so it cannot change a verified
//! artifact or the reference semantics.
//!
//! For a `for i in lo..hi` (or `lo..=hi`) loop whose body indexes a sequence
//! or slice `s` with the loop variable, `s[i]`, `s.set(i, v)`,
//! `s.try_get(i)` and `s.try_set(i, v)` check `i < s.len()` on every
//! iteration. When nothing in the loop can change the length of `s`, the
//! pass versions the loop:
//!
//! ```text
//! preheader: ...; if bounds_guard(lo, hi, s..) { fast loop } else { loop }
//! ```
//!
//! The guard holds when the range is empty or `hi <= s.len()` (`hi <
//! s.len()` for `..=`) for every hoisted `s`; the loop index only takes
//! values in `lo..hi`, so every hoisted access is then in bounds and the
//! fast copy uses [`SeqOp::Hoisted`] accesses without a check. When the guard
//! fails, the original, checked loop runs unchanged, so a short sequence
//! faults at the same iteration with the same prior effects.
//!
//! The pass is conservative. It skips a loop that contains a closure, a
//! handler or an indirect call, carries Box drop or reuse bookkeeping, lets
//! a value defined inside it escape, or is entered other than through its
//! header. It hoists only sequences named by a place that no instruction in
//! the loop writes, rebinds, moves or passes other than `in`, except element
//! accesses (`get`/`set`/`try_get`/`try_set`), which never resize.
use crate::ast::{Pass, PatternKind, Type};
use crate::check::Place;
use crate::ir::{Argument, FunctionIr, Instruction, Op, Program, Terminator, ValueId};
use crate::seq::{Access, SeqOp};
use std::collections::{BTreeMap, BTreeSet};

/// Versions every eligible counted loop of `program`; returns how many.
pub fn run(program: &mut Program) -> usize {
    program.functions.values_mut().map(hoist_function).sum()
}

/// Versions the eligible loops of one function.
pub fn hoist_function(f: &mut FunctionIr) -> usize {
    let original = f.blocks.len();
    (0..original).filter(|&h| version(f, h)).count()
}

/// The counted-loop header shape [`crate::loops`] lowers `for` to.
struct Header {
    index: String,
    bound: String,
    inclusive: bool,
    body: usize,
}

fn header(f: &FunctionIr, h: usize) -> Option<Header> {
    let b = &f.blocks[h];
    let [first, i, n, test] = b.instructions.as_slice() else {
        return None;
    };
    if !matches!(first.op, Op::Loop) {
        return None;
    }
    let slot = |inst: &Instruction, suffix: &str| match &inst.op {
        Op::Read { root, path, .. }
            if path.is_empty() && root.starts_with("$loop") && root.ends_with(suffix) =>
        {
            Some(root.clone())
        }
        _ => None,
    };
    let index = slot(i, "_index")?;
    let bound = slot(n, "_bound")?;
    let Op::Binary(op, a, c) = &test.op else {
        return None;
    };
    let inclusive = match op.as_str() {
        "<" => false,
        "<=" => true,
        _ => return None,
    };
    let u64_value = |v: ValueId| {
        f.values
            .get(&v)
            .is_some_and(|t| crate::numeric::default_type(t) == Type::named("u64"))
    };
    if *a != i.dest || *c != n.dest || !u64_value(i.dest) || !u64_value(n.dest) {
        return None;
    }
    let Terminator::Branch { condition, yes, .. } = b.terminator else {
        return None;
    };
    (condition == test.dest).then_some(Header {
        index,
        bound,
        inclusive,
        body: yes,
    })
}

fn successors(t: &Terminator) -> Vec<usize> {
    match t {
        Terminator::Jump { block, .. } => vec![*block],
        Terminator::Branch { yes, no, .. } => vec![*yes, *no],
        Terminator::Match { arms, .. } => arms.iter().map(|(_, b)| *b).collect(),
        Terminator::Return(_) | Terminator::Unreachable => vec![],
    }
}

/// The value operands of `op`, mutably.
fn arguments(args: &mut [Argument]) -> Vec<&mut ValueId> {
    args.iter_mut().map(|a| &mut a.value).collect()
}

fn operands(op: &mut Op) -> Vec<&mut ValueId> {
    match op {
        Op::Constant(_) | Op::Unit | Op::Read { .. } | Op::Loop | Op::Closure { .. } => vec![],
        Op::Bind { value, .. }
        | Op::Write { value, .. }
        | Op::Unary(_, value)
        | Op::Try(value)
        | Op::Project(value, _)
        | Op::Exit { value, .. } => vec![value],
        Op::Binary(_, a, b) | Op::Index(a, b) => vec![a, b],
        Op::Tuple(vs) | Op::Vector { args: vs, .. } => vs.iter_mut().collect(),
        Op::Data(_, fields) => fields.iter_mut().map(|(_, v)| v).collect(),
        Op::Call { args: a, .. }
        | Op::Tiled { args: a, .. }
        | Op::Seq { args: a, .. }
        | Op::Fip { args: a, .. } => arguments(a),
        Op::Indirect { callee, args: a } => {
            let mut out: Vec<&mut ValueId> = vec![callee];
            out.extend(a.iter_mut().map(|a| &mut a.value));
            out
        }
        Op::Handle { state_value, .. } => state_value.iter_mut().collect(),
    }
}

fn terminator_operands(t: &mut Terminator) -> Vec<&mut ValueId> {
    match t {
        Terminator::Return(v) => vec![v],
        Terminator::Jump { args, .. } => args.iter_mut().collect(),
        Terminator::Branch { condition, .. } => vec![condition],
        Terminator::Match { value, .. } => vec![value],
        Terminator::Unreachable => vec![],
    }
}

fn uses(op: &Op) -> Vec<ValueId> {
    operands(&mut op.clone()).into_iter().map(|v| *v).collect()
}

fn terminator_uses(t: &Terminator) -> Vec<ValueId> {
    terminator_operands(&mut t.clone())
        .into_iter()
        .map(|v| *v)
        .collect()
}

/// The blocks of the natural loop of header `h`: those reachable from `h`
/// that reach `h` again, with `h`.
fn natural_loop(f: &FunctionIr, h: usize, preds: &[Vec<usize>]) -> BTreeSet<usize> {
    let mut forward = BTreeSet::new();
    let mut stack = successors(&f.blocks[h].terminator);
    while let Some(b) = stack.pop() {
        if b != h && forward.insert(b) {
            stack.extend(successors(&f.blocks[b].terminator));
        }
    }
    let mut backward = BTreeSet::new();
    let mut stack = preds[h].clone();
    while let Some(b) = stack.pop() {
        if b != h && backward.insert(b) {
            stack.extend(preds[b].iter().copied());
        }
    }
    let mut blocks: BTreeSet<usize> = forward.intersection(&backward).copied().collect();
    blocks.insert(h);
    blocks
}

fn binds(pattern: &crate::ast::Pattern, name: &str) -> bool {
    match &pattern.kind {
        PatternKind::Bind(n) => n == name,
        PatternKind::Tuple(ps) => ps.iter().any(|p| binds(p, name)),
        PatternKind::Constructor { fields, .. } => fields.iter().any(|(_, p)| binds(p, name)),
        PatternKind::Wild | PatternKind::Literal(_) => false,
    }
}

fn place_root(place: &str) -> &str {
    place.split('.').next().unwrap_or(place)
}

/// The access a checked element operation becomes in the fast copy.
fn hoistable(op: SeqOp) -> Option<Access> {
    Some(match op {
        SeqOp::Get | SeqOp::SliceGet => Access::Get,
        SeqOp::Set | SeqOp::SliceSet => Access::Set,
        SeqOp::TryGet => Access::TryGet,
        SeqOp::TrySet => Access::TrySet,
        _ => None?,
    })
}

/// Operations on a sequence that never change its length.
fn keeps_length(op: SeqOp) -> bool {
    hoistable(op).is_some()
        || matches!(
            op,
            SeqOp::Len | SeqOp::SliceLen | SeqOp::AsSlice | SeqOp::SubSlice
        )
}

/// Versions the loop headed by `h` when it is eligible.
fn version(f: &mut FunctionIr, h: usize) -> bool {
    let Some(head) = header(f, h) else {
        return false;
    };
    let mut preds = vec![vec![]; f.blocks.len()];
    for (b, block) in f.blocks.iter().enumerate() {
        for s in successors(&block.terminator) {
            if let Some(p) = preds.get_mut(s) {
                p.push(b);
            }
        }
    }
    let blocks = natural_loop(f, h, &preds);
    if !blocks.contains(&head.body) {
        return false;
    }
    // One preheader, entering through the header only.
    let outside: Vec<usize> = preds[h]
        .iter()
        .copied()
        .filter(|p| !blocks.contains(p))
        .collect();
    let [pre] = outside.as_slice() else {
        return false;
    };
    let pre = *pre;
    if !matches!(&f.blocks[pre].terminator, Terminator::Jump { block, args } if *block == h && args.is_empty())
    {
        return false;
    }
    for &b in &blocks {
        if b != h && preds[b].iter().any(|p| !blocks.contains(p)) {
            return false;
        }
        if f.fip_join_slots.contains_key(&b)
            || f.box_drops_before_inst.contains_key(&b)
            || f.box_drops_after_inst.contains_key(&b)
            || f.box_drops_before_term.contains_key(&b)
        {
            return false;
        }
    }
    let insts = || {
        blocks
            .iter()
            .flat_map(|&b| f.blocks[b].instructions.iter().map(move |i| (b, i)))
    };
    if insts().any(|(_, i)| {
        matches!(
            i.op,
            Op::Closure { .. } | Op::Handle { .. } | Op::Indirect { .. }
        ) || f.fip_call_slots.contains_key(&i.dest)
    }) {
        return false;
    }
    // No value defined in the loop is used outside it.
    let mut defined: BTreeSet<ValueId> = insts().map(|(_, i)| i.dest).collect();
    for &b in &blocks {
        defined.extend(f.blocks[b].params.iter().copied());
    }
    for (b, block) in f.blocks.iter().enumerate() {
        if blocks.contains(&b) {
            continue;
        }
        let used = block
            .instructions
            .iter()
            .flat_map(|i| uses(&i.op))
            .chain(terminator_uses(&block.terminator));
        if used.into_iter().any(|v| defined.contains(&v)) {
            return false;
        }
    }
    // The bound never changes; the index changes only in latch blocks that
    // jump straight back to the header.
    let mut latches = BTreeSet::new();
    for (b, i) in insts() {
        match &i.op {
            Op::Write { place, .. } if place_root(place) == head.bound => return false,
            Op::Write { place, .. } if place_root(place) == head.index => {
                if !matches!(f.blocks[b].terminator, Terminator::Jump { block, .. } if block == h) {
                    return false;
                }
                latches.insert(b);
            }
            Op::Bind { pattern, .. }
                if binds(pattern, &head.index) || binds(pattern, &head.bound) =>
            {
                return false;
            }
            _ => {}
        }
    }
    // Values that hold an index of the range: reads of the index slot after
    // the header test, and reads of a local bound once from one.
    let mut induction: BTreeSet<ValueId> = BTreeSet::new();
    for (b, i) in insts() {
        if let Op::Read { root, path, .. } = &i.op
            && b != h
            && path.is_empty()
            && *root == head.index
        {
            induction.insert(i.dest);
        }
    }
    let mut locals = BTreeSet::new();
    for (_, i) in insts() {
        if let Op::Bind { pattern, value } = &i.op
            && let PatternKind::Bind(name) = &pattern.kind
            && induction.contains(value)
            && single_binding(f, name)
        {
            locals.insert(name.clone());
        }
    }
    for (b, i) in insts() {
        if let Op::Read { root, path, .. } = &i.op
            && b != h
            && path.is_empty()
            && locals.contains(root)
        {
            induction.insert(i.dest);
        }
    }
    // Candidate accesses, by receiver place and index offset.
    let reads: BTreeMap<ValueId, &Instruction> = insts()
        .filter(|(_, i)| matches!(i.op, Op::Read { .. }))
        .map(|(_, i)| (i.dest, i))
        .collect();
    let sums: BTreeMap<ValueId, (ValueId, ValueId)> = insts()
        .filter_map(|(_, i)| match &i.op {
            Op::Binary(op, a, b) if op == "+" || op == "wrapping_add" => Some((i.dest, (*a, *b))),
            _ => None,
        })
        .collect();
    let is_u64 = |v: ValueId| {
        f.values
            .get(&v)
            .is_some_and(|t| crate::numeric::default_type(t) == Type::named("u64"))
    };
    // A loop-invariant `u64` offset: a value defined before the loop, or a
    // read of a local the loop never changes.
    let invariant = |v: ValueId| -> Option<Offset> {
        if !is_u64(v) {
            return None;
        }
        if !defined.contains(&v) {
            return Some(Offset::Value(v));
        }
        let read = reads.get(&v)?;
        match &read.op {
            Op::Read { root, path, .. } if path.is_empty() && invariant_local(f, &blocks, root) => {
                Some(Offset::Local(Box::new((*read).clone())))
            }
            _ => None,
        }
    };
    let mut receivers: BTreeMap<String, Receiver> = BTreeMap::new();
    let mut candidates: BTreeMap<ValueId, (Access, String)> = BTreeMap::new();
    for (b, i) in insts() {
        let Op::Seq { op, args } = &i.op else {
            continue;
        };
        let (Some(access), Some(receiver), Some(index)) =
            (hoistable(*op), args.first(), args.get(1))
        else {
            continue;
        };
        let (Some(place), Some(read)) = (&receiver.place, reads.get(&receiver.value)) else {
            continue;
        };
        if latches.contains(&b) {
            continue;
        }
        let offset = if induction.contains(&index.value) {
            Offset::Zero
        } else if let Some((x, y)) = sums.get(&index.value) {
            let found = if induction.contains(x) {
                invariant(*y)
            } else if induction.contains(y) {
                invariant(*x)
            } else {
                None
            };
            match found {
                Some(offset) => offset,
                None => continue,
            }
        } else {
            continue;
        };
        let Op::Read { root, path, .. } = &read.op else {
            continue;
        };
        if root != &place.root || path != &place.path {
            continue;
        }
        let key = format!("{}+{}", place.display(), offset.key());
        receivers.entry(key.clone()).or_insert_with(|| Receiver {
            place: place.clone(),
            read: (*read).clone(),
            offset,
        });
        candidates.insert(i.dest, (access, key));
    }
    // Keep only receivers whose length the loop cannot change.
    receivers.retain(|_, r| stable(f, &blocks, &r.place.root));
    candidates.retain(|_, (_, key)| receivers.contains_key(key));
    if candidates.is_empty() {
        return false;
    }
    let receivers: Vec<Receiver> = receivers.into_values().collect();
    clone_loop(f, h, pre, &head, &blocks, &receivers, &candidates);
    true
}

/// The loop-invariant offset `c` of an access `s[c + i]`.
enum Offset {
    Zero,
    /// A value defined before the loop.
    Value(ValueId),
    /// A read of a local the loop never changes, repeated in the preheader.
    Local(Box<Instruction>),
}

impl Offset {
    fn key(&self) -> String {
        match self {
            Self::Zero => "0".into(),
            Self::Value(v) => format!("%{v}"),
            Self::Local(read) => match &read.op {
                Op::Read { root, .. } => root.clone(),
                _ => String::new(),
            },
        }
    }
}

/// A hoisted receiver: its place, the read of it, and the index offset.
struct Receiver {
    place: Place,
    read: Instruction,
    offset: Offset,
}

/// Whether the loop never writes, rebinds or lends `inout` the local `root`.
fn invariant_local(f: &FunctionIr, blocks: &BTreeSet<usize>, root: &str) -> bool {
    blocks.iter().all(|&b| {
        f.blocks[b].instructions.iter().all(|i| {
            let lent = |args: &[Argument]| {
                args.iter().any(|a| {
                    a.pass != Pass::In
                        && a.pass != Pass::Sink
                        && a.place.as_ref().is_some_and(|p| p.root == root)
                })
            };
            match &i.op {
                Op::Write { place, .. } => place_root(place) != root,
                Op::Bind { pattern, .. } => !binds(pattern, root),
                Op::Call { args, .. }
                | Op::Indirect { args, .. }
                | Op::Seq { args, .. }
                | Op::Tiled { args, .. }
                | Op::Fip { args, .. } => !lent(args),
                _ => true,
            }
        })
    })
}

/// Whether `name` is bound by exactly one `Bind`, never written, and never
/// lent to an operation other than as `in` or by value.
fn single_binding(f: &FunctionIr, name: &str) -> bool {
    let lent = |args: &[Argument]| {
        args.iter().any(|a| {
            a.pass != Pass::In
                && a.pass != Pass::Sink
                && a.place.as_ref().is_some_and(|p| p.root == name)
        })
    };
    let mut count = 0;
    for i in f.blocks.iter().flat_map(|b| &b.instructions) {
        match &i.op {
            Op::Bind { pattern, .. } if binds(pattern, name) => count += 1,
            Op::Write { place, .. } if place_root(place) == name => return false,
            Op::Call { args, .. }
            | Op::Indirect { args, .. }
            | Op::Seq { args, .. }
            | Op::Tiled { args, .. }
            | Op::Fip { args, .. }
                if lent(args) =>
            {
                return false;
            }
            Op::Closure { .. } | Op::Handle { .. } => return false,
            _ => {}
        }
    }
    count == 1
}

/// Whether nothing in `blocks` can change the length of a sequence rooted
/// at `root`.
fn stable(f: &FunctionIr, blocks: &BTreeSet<usize>, root: &str) -> bool {
    for &b in blocks {
        for i in &f.blocks[b].instructions {
            let rooted = |a: &Argument| a.place.as_ref().is_some_and(|p| p.root == root);
            let ok = match &i.op {
                Op::Write { place, .. } => place_root(place) != root,
                Op::Bind { pattern, .. } => !binds(pattern, root),
                Op::Read { root: r, pass, .. } => r != root || *pass != Pass::Sink,
                Op::Seq { op, args } => {
                    let receiver = args.first().is_some_and(rooted) && op.has_receiver();
                    let others = if op.has_receiver() {
                        &args[1..]
                    } else {
                        &args[..]
                    };
                    (!receiver || keeps_length(*op))
                        && others.iter().all(|a| !rooted(a) || a.pass == Pass::In)
                }
                Op::Call { args, .. } | Op::Tiled { args, .. } | Op::Fip { args, .. } => {
                    args.iter().all(|a| !rooted(a) || a.pass == Pass::In)
                }
                _ => true,
            };
            if !ok {
                return false;
            }
        }
    }
    true
}

/// Adds the guard to `pre` and a fast copy of the loop `blocks`.
fn clone_loop(
    f: &mut FunctionIr,
    h: usize,
    pre: usize,
    head: &Header,
    blocks: &BTreeSet<usize>,
    receivers: &[Receiver],
    candidates: &BTreeMap<ValueId, (Access, String)>,
) {
    let mut next_value = f
        .values
        .keys()
        .copied()
        .chain(f.blocks.iter().flat_map(|b| {
            b.instructions
                .iter()
                .map(|i| i.dest)
                .chain(b.params.iter().copied())
        }))
        .max()
        .map_or(0, |v| v + 1);
    let mut next_token = f
        .blocks
        .iter()
        .flat_map(|b| {
            [b.entry_token, b.outgoing_token]
                .into_iter()
                .chain(b.incoming_tokens.iter().map(|(_, t)| *t))
                .chain(
                    b.instructions
                        .iter()
                        .filter_map(|i| i.token)
                        .flat_map(|(a, b)| [a, b]),
                )
        })
        .max()
        .map_or(0, |t| t + 1);
    let mut fresh_value = |f: &mut FunctionIr, ty: Type| {
        let v = next_value;
        next_value += 1;
        f.values.insert(v, ty);
        v
    };
    // The guard, at the end of the preheader.
    let span = f.blocks[h].instructions[0].span.clone();
    // The zero-width span the checker typed as the range's element type,
    // which a synthesized `u64` constant needs.
    let at = f.blocks[h].instructions[1].span.clone();
    let before = f.blocks[pre].outgoing_token;
    let guard_token = next_token;
    next_token += 1;
    let u64_ty = Type::named("u64");
    let read_slot = |dest, root: &str| Instruction {
        dest,
        op: Op::Read {
            root: root.into(),
            path: vec![],
            pass: Pass::Sink,
            projections: vec![],
        },
        ty: u64_ty.clone(),
        span: span.clone(),
        token: Some((before, before)),
        fallible: false,
    };
    let lo = fresh_value(f, u64_ty.clone());
    let hi = fresh_value(f, u64_ty.clone());
    let mut guard_insts = vec![read_slot(lo, &head.index), read_slot(hi, &head.bound)];
    let mut guard_args = vec![
        Argument {
            pass: Pass::Sink,
            value: lo,
            place: None,
        },
        Argument {
            pass: Pass::Sink,
            value: hi,
            place: None,
        },
    ];
    for r in receivers {
        let v = fresh_value(f, r.read.ty.clone());
        guard_insts.push(Instruction {
            dest: v,
            span: span.clone(),
            token: Some((before, before)),
            fallible: false,
            ..r.read.clone()
        });
        let offset = match &r.offset {
            Offset::Zero => {
                let c = fresh_value(f, u64_ty.clone());
                guard_insts.push(Instruction {
                    dest: c,
                    op: Op::Constant(crate::ast::Literal::Int(0)),
                    ty: u64_ty.clone(),
                    span: at.clone(),
                    token: None,
                    fallible: false,
                });
                c
            }
            Offset::Value(c) => *c,
            Offset::Local(read) => {
                let c = fresh_value(f, u64_ty.clone());
                guard_insts.push(Instruction {
                    dest: c,
                    span: span.clone(),
                    token: Some((before, before)),
                    fallible: false,
                    ..(**read).clone()
                });
                c
            }
        };
        guard_args.push(Argument {
            pass: Pass::In,
            value: v,
            place: Some(r.place.clone()),
        });
        guard_args.push(Argument {
            pass: Pass::Sink,
            value: offset,
            place: None,
        });
    }
    let guard = fresh_value(f, Type::named("bool"));
    guard_insts.push(Instruction {
        dest: guard,
        op: Op::Seq {
            op: SeqOp::BoundsGuard {
                inclusive: head.inclusive,
            },
            args: guard_args,
        },
        ty: Type::named("bool"),
        span: span.clone(),
        token: Some((before, guard_token)),
        fallible: false,
    });
    // Fresh blocks, block parameters and tokens for the copy. Instructions
    // keep their value ids: the native backend keeps each IR value in one
    // frontend variable, so a fresh id per copied value would add a
    // loop-carried variable per value to every block of the function. The
    // two copies are disjoint paths and no loop value is used outside the
    // loop, so every use still sees the definition of its own copy.
    let base = f.blocks.len();
    let block_map: BTreeMap<usize, usize> = blocks
        .iter()
        .enumerate()
        .map(|(k, &b)| (b, base + k))
        .collect();
    let mut value_map: BTreeMap<ValueId, ValueId> = BTreeMap::new();
    let mut token_map: BTreeMap<u32, u32> = BTreeMap::new();
    for &b in blocks {
        let block = &f.blocks[b];
        let defs: Vec<ValueId> = block.params.clone();
        let mut tokens = vec![block.entry_token];
        tokens.extend(
            block
                .instructions
                .iter()
                .filter_map(|i| i.token)
                .filter(|(a, b)| a != b)
                .map(|(_, b)| b),
        );
        for v in defs {
            let ty = f.values.get(&v).cloned().unwrap_or(Type::Infer);
            let n = fresh_value(f, ty);
            value_map.insert(v, n);
        }
        for t in tokens {
            token_map.insert(t, next_token);
            next_token += 1;
        }
    }
    let map_value = |v: &mut ValueId| {
        if let Some(n) = value_map.get(v) {
            *v = *n;
        }
    };
    let map_token = |t: u32| token_map.get(&t).copied().unwrap_or(t);
    let mut copies = vec![];
    for &b in blocks {
        let mut block = f.blocks[b].clone();
        block.params.iter_mut().for_each(map_value);
        block.entry_token = map_token(block.entry_token);
        block.outgoing_token = map_token(block.outgoing_token);
        block.incoming_tokens = block
            .incoming_tokens
            .iter()
            .filter(|(p, _)| blocks.contains(p))
            .map(|(p, t)| (block_map[p], map_token(*t)))
            .collect();
        if b == h {
            block.incoming_tokens.push((pre, guard_token));
        }
        for i in &mut block.instructions {
            if let Some((access, _)) = candidates.get(&i.dest)
                && let Op::Seq { op, .. } = &mut i.op
            {
                *op = SeqOp::Hoisted(*access);
                i.fallible = false;
            }
            map_value(&mut i.dest);
            operands(&mut i.op).into_iter().for_each(map_value);
            i.token = i.token.map(|(a, b)| (map_token(a), map_token(b)));
        }
        terminator_operands(&mut block.terminator)
            .into_iter()
            .for_each(map_value);
        match &mut block.terminator {
            Terminator::Jump { block: t, .. } => *t = block_map.get(t).copied().unwrap_or(*t),
            Terminator::Branch { yes, no, .. } => {
                *yes = block_map.get(yes).copied().unwrap_or(*yes);
                *no = block_map.get(no).copied().unwrap_or(*no);
            }
            Terminator::Match { arms, .. } => {
                for (_, t) in arms {
                    *t = block_map.get(t).copied().unwrap_or(*t);
                }
            }
            Terminator::Return(_) | Terminator::Unreachable => {}
        }
        copies.push(block);
    }
    // Exits gain the copy's edges.
    let mut exit_edges: Vec<(usize, usize, u32)> = vec![];
    for (&b, copy) in blocks.iter().zip(&copies) {
        for s in successors(&copy.terminator) {
            if s < base {
                exit_edges.push((s, block_map[&b], copy.outgoing_token));
            }
        }
    }
    f.blocks.extend(copies);
    for (s, from, token) in exit_edges {
        f.blocks[s].incoming_tokens.push((from, token));
    }
    for entry in &mut f.blocks[h].incoming_tokens {
        if entry.0 == pre {
            entry.1 = guard_token;
        }
    }
    let pre_block = &mut f.blocks[pre];
    pre_block.instructions.extend(guard_insts);
    pre_block.outgoing_token = guard_token;
    pre_block.terminator = Terminator::Branch {
        condition: guard,
        yes: block_map[&h],
        no: h,
    };
}

/// A sequence header read once before a loop (port gaps lane 7, #990).
///
/// Element accesses in a loop read the receiver's handle and its `{len,
/// data}` words on every iteration, even when the loop cannot change them.
/// [`header_hoists`] finds, for each loop, the receiver places that no
/// instruction of the loop writes, rebinds, moves, resizes or lends other
/// than `in`; the native backend reads such a receiver and its header words
/// once at the end of `preheader` and gives the loop's `reads` of it those
/// values. Unlike [`run`] this needs no counted loop and no guard: element
/// accesses still check their bounds, against the hoisted length.
#[derive(Clone, Debug)]
pub struct HeaderHoist {
    /// The block that enters the loop (it dominates every loop block).
    pub preheader: usize,
    /// The receiver place.
    pub place: Place,
    /// The receiver's sequence or slice type.
    pub ty: Type,
    /// The reads of `place` in the loop that only receivers of accesses use.
    pub reads: BTreeSet<ValueId>,
}

/// The hoistable sequence headers of `f` ([`HeaderHoist`]); each read is
/// hoisted out of the outermost loop that keeps it stable.
pub fn header_hoists(f: &FunctionIr) -> Vec<HeaderHoist> {
    if !f.fip_join_slots.is_empty()
        || !f.box_drops_before_inst.is_empty()
        || !f.box_drops_after_inst.is_empty()
        || !f.box_drops_before_term.is_empty()
        || !f.fip_call_slots.is_empty()
    {
        return vec![];
    }
    // Predecessors among the blocks the entry reaches (a block after a
    // `break` or `return` has none and enters nothing).
    let mut reached = BTreeSet::from([0]);
    let mut stack = vec![0];
    while let Some(b) = stack.pop() {
        for s in successors(&f.blocks[b].terminator) {
            if s < f.blocks.len() && reached.insert(s) {
                stack.push(s);
            }
        }
    }
    let mut preds = vec![vec![]; f.blocks.len()];
    for &b in &reached {
        for s in successors(&f.blocks[b].terminator) {
            if let Some(p) = preds.get_mut(s) {
                p.push(b);
            }
        }
    }
    // Uses of each value, by block.
    let mut users: BTreeMap<ValueId, Vec<(usize, Option<&Instruction>)>> = BTreeMap::new();
    for (b, block) in f.blocks.iter().enumerate() {
        for i in &block.instructions {
            for v in uses(&i.op) {
                users.entry(v).or_default().push((b, Some(i)));
            }
        }
        for v in terminator_uses(&block.terminator) {
            users.entry(v).or_default().push((b, None));
        }
    }
    let mut found: Vec<(usize, HeaderHoist)> = vec![];
    for &h in &reached {
        let blocks = natural_loop(f, h, &preds);
        if !preds[h].iter().any(|p| blocks.contains(p)) {
            continue;
        }
        let outside: Vec<usize> = preds[h]
            .iter()
            .copied()
            .filter(|p| !blocks.contains(p))
            .collect();
        let [pre] = outside.as_slice() else {
            continue;
        };
        if blocks
            .iter()
            .any(|&b| b != h && preds[b].iter().any(|p| !blocks.contains(p)))
        {
            continue;
        }
        let insts = || blocks.iter().flat_map(|&b| f.blocks[b].instructions.iter());
        if insts().any(|i| match &i.op {
            Op::Closure { .. } | Op::Handle { .. } | Op::Indirect { .. } => true,
            // An effect operation runs a handler outside the loop's view.
            Op::Call { callee, .. } => callee.contains("::"),
            _ => false,
        }) {
            continue;
        }
        let mut places: BTreeMap<String, HeaderHoist> = BTreeMap::new();
        for i in insts() {
            let Op::Read {
                root,
                path,
                pass: Pass::In,
                ..
            } = &i.op
            else {
                continue;
            };
            let ty = crate::numeric::default_type(&i.ty);
            if !(crate::seq::is_seq_type(&ty) || crate::seq::is_slice_type(&ty))
                || crate::strings::is_text(&ty)
            {
                continue;
            }
            let place = Place {
                root: root.clone(),
                path: path.clone(),
            };
            // Every use is the receiver of an access in the loop.
            let only_receivers = users.get(&i.dest).is_some_and(|us| {
                us.iter().all(|(b, user)| {
                    blocks.contains(b)
                        && user.is_some_and(|u| match &u.op {
                            Op::Seq { op, args } => {
                                op.has_receiver()
                                    && (keeps_length(*op) || matches!(op, SeqOp::Hoisted(_)))
                                    && args.first().is_some_and(|a| {
                                        a.value == i.dest
                                            && a.pass == Pass::In
                                            && a.place.as_ref() == Some(&place)
                                    })
                                    && args[1..].iter().all(|a| a.value != i.dest)
                            }
                            _ => false,
                        })
                })
            });
            if !only_receivers || !stable_place(f, &blocks, &place) {
                continue;
            }
            places
                .entry(place.display())
                .or_insert_with(|| HeaderHoist {
                    preheader: *pre,
                    place: place.clone(),
                    ty: ty.clone(),
                    reads: BTreeSet::new(),
                })
                .reads
                .insert(i.dest);
        }
        found.extend(places.into_values().map(|hoist| (blocks.len(), hoist)));
    }
    // Outermost loop first; a read claimed by an enclosing loop is skipped.
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut claimed = BTreeSet::new();
    let mut out: Vec<HeaderHoist> = vec![];
    for (_, mut hoist) in found {
        hoist.reads.retain(|r| !claimed.contains(r));
        if hoist.reads.is_empty() {
            continue;
        }
        claimed.extend(hoist.reads.iter().copied());
        match out
            .iter_mut()
            .find(|o| o.preheader == hoist.preheader && o.place == hoist.place)
        {
            Some(o) => o.reads.extend(hoist.reads),
            None => out.push(hoist),
        }
    }
    out
}

fn parse_place(place: &str) -> Place {
    let mut parts = place.split('.');
    Place {
        root: parts.next().unwrap_or_default().into(),
        path: parts.map(str::to_owned).collect(),
    }
}

/// Whether nothing in `blocks` can change the handle or the `{len, data}`
/// header of the sequence at `place`: no write, binding or move of a place
/// that overlaps it, no operation that resizes it, and no loan of it other
/// than `in`. Writing a disjoint field of the same root is allowed.
fn stable_place(f: &FunctionIr, blocks: &BTreeSet<usize>, place: &Place) -> bool {
    let lent =
        |a: &Argument| a.pass != Pass::In && a.place.as_ref().is_some_and(|p| p.overlaps(place));
    blocks.iter().all(|&b| {
        f.blocks[b].instructions.iter().all(|i| match &i.op {
            Op::Write { place: w, .. } => !parse_place(w).overlaps(place),
            Op::Bind { pattern, .. } => !binds(pattern, &place.root),
            Op::Read {
                root, path, pass, ..
            } => {
                *pass != Pass::Sink
                    || !Place {
                        root: root.clone(),
                        path: path.clone(),
                    }
                    .overlaps(place)
            }
            Op::Seq { op, args } => {
                let receiver = op.has_receiver()
                    && args
                        .first()
                        .is_some_and(|a| a.place.as_ref().is_some_and(|p| p.overlaps(place)));
                let others = if op.has_receiver() && !args.is_empty() {
                    &args[1..]
                } else {
                    &args[..]
                };
                (!receiver || keeps_length(*op) || matches!(op, SeqOp::Hoisted(_)))
                    && !others.iter().any(lent)
            }
            Op::Call { args, .. } | Op::Tiled { args, .. } | Op::Fip { args, .. } => {
                !args.iter().any(lent)
            }
            Op::Closure { .. } | Op::Handle { .. } | Op::Indirect { .. } => false,
            _ => true,
        })
    })
}
