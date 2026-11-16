//! Checked Place IR to the slot IR (`Lir`): plan the reachable functions,
//! flatten every value of the scalar subset into 64-bit words and reject
//! everything else with a typed error.
use super::machine::{
    ArithOp, Cond, FIRST_FREE_LABEL, FloatOp, Kind, Label, Lir, Slot, TrapKind, WrapOp,
};
use super::{Unsupported, VerifiedError};
use crate::ast::{Literal, Pass, Pattern, PatternKind, Type, substitute};
use crate::check::World;
use crate::ir::{Block, FunctionIr, Instruction, Op, Program, Terminator, ValueId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The flattened words of one value: `(field path, kind)` per word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shape(pub Vec<(Vec<String>, Kind)>);
impl Shape {
    pub fn len(&self) -> u32 {
        self.0.len() as u32
    }
    /// The word range of the sub-value at `path`.
    fn range(&self, path: &[String]) -> Option<(u32, u32)> {
        let mut start = None;
        let mut end = 0;
        for (i, (p, _)) in self.0.iter().enumerate() {
            if p.starts_with(path) {
                if start.is_none() {
                    start = Some(i as u32);
                }
                end = i as u32 + 1;
            }
        }
        match start {
            Some(s) => Some((s, end - s)),
            // A zero-word sub-value (Unit, or an empty struct).
            None => Some((self.0.len() as u32, 0)),
        }
    }
}

/// The flattening of a type: integers and `bool` are one word, `Unit` none,
/// non-generic-after-substitution structs and tuples the concatenation of
/// their fields in declaration order.
pub fn shape(world: &World, ty: &Type, function: &str) -> Result<Shape, VerifiedError> {
    let mut out = vec![];
    flatten(world, ty, &mut vec![], &mut out, function, 0, &mut vec![])?;
    Ok(Shape(out))
}

/// Path of an enum's tag word.
pub const TAG: &str = "#tag";
/// Path prefix of an enum's payload words.
pub const PAYLOAD: &str = "#payload";

/// The generic substitution of `adt` instantiated at `args`.
fn adt_vars(adt: &crate::ast::Adt, args: &[Type]) -> BTreeMap<String, Type> {
    adt.generics
        .iter()
        .zip(args)
        .map(|(g, t)| (g.name.clone(), t.clone()))
        .collect()
}

/// One variant of an enum value's layout: its tag and, per field, the
/// field's name, its word offset from the start of the value (after the tag
/// word) and its substituted type.
pub struct VariantLayout {
    pub tag: u64,
    pub fields: Vec<(String, u32, Type)>,
}

/// The layout of variant `ctor` (a short or `Enum::Variant` name) of the
/// enum type `ty`, or `None` when `ty` is not an enum.
pub fn variant_layout(
    world: &World,
    ty: &Type,
    ctor: &str,
    function: &str,
) -> Option<Result<VariantLayout, VerifiedError>> {
    let ty = crate::numeric::default_type(ty);
    let Type::Named(n, args) = &ty else {
        return None;
    };
    let adt = world.adts.get(n).filter(|a| a.is_enum)?;
    let short = ctor.rsplit("::").next().unwrap_or(ctor);
    let Some((tag, variant)) = adt
        .variants
        .iter()
        .enumerate()
        .find(|(_, v)| v.name == short)
    else {
        return Some(Err(VerifiedError::Invalid(format!(
            "{ctor} is not a variant of {ty}"
        ))));
    };
    let vars = adt_vars(adt, args);
    let mut offset = 1;
    let mut fields = vec![];
    for f in &variant.fields {
        let t = substitute(&f.ty, &vars);
        let words = match shape(world, &t, function) {
            Ok(s) => s.len(),
            Err(e) => return Some(Err(e)),
        };
        fields.push((f.name.clone(), offset, t));
        offset += words;
    }
    Some(Ok(VariantLayout {
        tag: tag as u64,
        fields,
    }))
}

fn flatten(
    world: &World,
    ty: &Type,
    path: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, Kind)>,
    function: &str,
    depth: usize,
    active: &mut Vec<String>,
) -> Result<(), VerifiedError> {
    if depth > 32 {
        return Err(VerifiedError::Limit(format!(
            "type nesting deeper than 32 in '{function}'"
        )));
    }
    let ty = crate::numeric::default_type(ty);
    let kind = match ty.name() {
        "u8" => Some(Kind::U8),
        "u32" => Some(Kind::U32),
        "u64" => Some(Kind::U64),
        "i64" => Some(Kind::I64),
        "bool" => Some(Kind::Bool),
        "f64" => Some(Kind::F64),
        _ => None,
    };
    if let Some(k) = kind {
        out.push((path.clone(), k));
        return Ok(());
    }
    match &ty {
        Type::Tuple(ts) => {
            for (i, t) in ts.iter().enumerate() {
                path.push(i.to_string());
                flatten(world, t, path, out, function, depth + 1, active)?;
                path.pop();
            }
            Ok(())
        }
        Type::Named(n, _) if n == "Unit" => Ok(()),
        // A region handle is the heap word index of its payload.
        Type::Named(n, _) if n == crate::regions::BOX_TYPE => {
            out.push((path.clone(), Kind::U64));
            Ok(())
        }
        Type::Named(n, _) if n == crate::regions::WATERMARK_TYPE => {
            Err(VerifiedError::unsupported(
                Unsupported::Effect,
                function,
                "region watermarks (checkpoint, rewind, commit)",
            ))
        }
        Type::Named(n, _) if n == "f32" => Err(VerifiedError::unsupported(
            Unsupported::Float,
            function,
            format!("type {ty}"),
        )),
        Type::Named(n, _) if n == "String" => Err(VerifiedError::unsupported(
            Unsupported::String,
            function,
            format!("type {ty}"),
        )),
        Type::Named(n, args) => {
            let Some(adt) = world.adts.get(n) else {
                return Err(VerifiedError::unsupported(
                    Unsupported::Type,
                    function,
                    format!("type {ty}"),
                ));
            };
            let vars = adt_vars(adt, args);
            if adt.is_enum {
                // A tag word, then a union payload as wide as the widest
                // variant. Payload words are untyped (`u64`): each variant
                // reads them at its own field kinds.
                // Recursion is the same instantiation reached inside itself;
                // finite nesting through arguments (`Option[Option[i64]]`)
                // is not recursion. Polymorphic recursion hits the depth
                // limit instead.
                let key = ty.to_string();
                if active.contains(&key) {
                    return Err(VerifiedError::unsupported(
                        Unsupported::Enum,
                        function,
                        format!("recursive enum {ty} (no heap in this backend)"),
                    ));
                }
                active.push(key);
                let mut widest = 0;
                for v in &adt.variants {
                    let mut words = vec![];
                    for f in &v.fields {
                        flatten(
                            world,
                            &substitute(&f.ty, &vars),
                            &mut vec![],
                            &mut words,
                            function,
                            depth + 1,
                            active,
                        )?;
                    }
                    widest = widest.max(words.len());
                }
                active.pop();
                path.push(TAG.into());
                out.push((path.clone(), Kind::U64));
                path.pop();
                for i in 0..widest {
                    path.push(PAYLOAD.into());
                    path.push(i.to_string());
                    out.push((path.clone(), Kind::U64));
                    path.pop();
                    path.pop();
                }
                return Ok(());
            }
            for f in &adt.variants[0].fields {
                path.push(f.name.clone());
                flatten(
                    world,
                    &substitute(&f.ty, &vars),
                    path,
                    out,
                    function,
                    depth + 1,
                    active,
                )?;
                path.pop();
            }
            Ok(())
        }
        Type::Function { .. } => Err(VerifiedError::unsupported(
            Unsupported::FunctionValue,
            function,
            format!("type {ty}"),
        )),
        _ => Err(VerifiedError::unsupported(
            Unsupported::Type,
            function,
            format!("type {ty}"),
        )),
    }
}

/// The program heap's global words (heap word indices): words in use beyond
/// the globals, the innermost open region's header, and the pin (see
/// `region_block`).
const G_TOP: u32 = 0;
const G_INNER: u32 = 1;
const G_PIN: u32 = 2;
const GLOBALS: u32 = 3;
/// Region header words: charged bytes, allocations, the previous innermost
/// header, the heap words in use when it opened, and the capacity in bytes.
const H_USED: u32 = 0;
const H_ALLOCS: u32 = 1;
const H_PREV: u32 = 2;
const H_START: u32 = 3;
const H_CAP: u32 = 4;
const HEADER: u32 = 5;
/// Largest payload in words (heap offsets stay inside one AArch64 scaled
/// 12-bit immediate).
const MAX_PAYLOAD_WORDS: u32 = 4000;

/// One planned function.
pub struct Planned {
    pub name: String,
    pub label: Label,
    pub params: Vec<(Pass, Shape)>,
    /// For each parameter, the word ranges `(start, len)` the caller copies
    /// back after the call: the declared projections of an `inout`
    /// parameter (`inout p.{x}`), or the whole value when it declares none.
    pub writeback: Vec<Vec<(u32, u32)>>,
    pub result: Shape,
    /// Region capabilities the function receives, one pointer word each
    /// after its parameters: the labels of its row's `r: Region` entries,
    /// and for a region block's body also the enclosing capabilities and the
    /// block's own label (innermost last).
    pub caps: Vec<String>,
}

/// The functions reachable from the entry, in discovery order.
pub struct Plan {
    pub functions: Vec<Planned>,
    pub index: BTreeMap<String, usize>,
}

fn effects_supported(world: &World, f: &FunctionIr) -> Result<(), VerifiedError> {
    if f.effects.tail.is_some() {
        return Err(VerifiedError::unsupported(
            Unsupported::Effect,
            &f.name,
            "open effect row",
        ));
    }
    for e in &f.effects.instances {
        // Region capabilities are passed as pointer words; a handler body's
        // row may name an enclosing block's capability by its internal label.
        if e.name == crate::regions::REGION_EFFECT && crate::regions::active(world) {
            continue;
        }
        let arg = e.args.first().map(|t| t.name()).unwrap_or("");
        let ok = e.name == "Fail"
            && e.label.is_none()
            && e.args.len() == 1
            && matches!(arg, "ArithOverflow" | "RuntimeError");
        if !ok {
            return Err(VerifiedError::unsupported(
                Unsupported::Effect,
                &f.name,
                format!(
                    "effect {}{} (only unhandled Fail[ArithOverflow] and Fail[RuntimeError] aborts)",
                    e.name,
                    if e.args.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "[{}]",
                            e.args
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                ),
            ));
        }
    }
    Ok(())
}

/// A labelled entry `r: Region` of the built-in region effect.
fn is_region_capability(world: &World, e: &crate::ast::Effect) -> bool {
    e.name == crate::regions::REGION_EFFECT
        && e.label.as_deref().is_some_and(|l| !l.starts_with('$'))
        && e.args.is_empty()
        && crate::regions::active(world)
}

/// The region capability labels of a function's own row, in row order.
fn row_caps(world: &World, f: &FunctionIr) -> Vec<String> {
    f.effects
        .instances
        .iter()
        .filter(|e| is_region_capability(world, e))
        .filter_map(|e| e.label.clone())
        .collect()
}

/// The region block an instruction opens: its label, capacity in bytes and
/// the name of its body's nested function in the instance `parent`.
pub(super) fn region_block(
    world: &World,
    op: &Op,
    parent: &str,
) -> Option<(String, usize, String)> {
    let Op::Handle { expr, .. } = op else {
        return None;
    };
    let crate::ast::ExprKind::Handle {
        effect, body, ret, ..
    } = &expr.kind
    else {
        return None;
    };
    if !crate::regions::active(world) {
        return None;
    }
    let capacity = crate::regions::block_capacity(effect, ret)?;
    let label = effect.label.clone()?;
    let body =
        super::mono::nested_instance(parent, &crate::ir::nested_name("handler_body", &body.span));
    Some((label, capacity, body))
}

pub fn plan(
    world: &World,
    program: &Program,
    entry: &str,
    max_functions: usize,
) -> Result<Plan, VerifiedError> {
    let mut functions = vec![];
    let mut index = BTreeMap::new();
    let mut queue = VecDeque::from([(entry.to_owned(), None::<Vec<String>>)]);
    let mut seen = BTreeSet::from([entry.to_owned()]);
    let mut label = FIRST_FREE_LABEL;
    while let Some((name, inherited)) = queue.pop_front() {
        if functions.len() >= max_functions {
            return Err(VerifiedError::Limit(format!(
                "more than {max_functions} reachable functions"
            )));
        }
        let f = program.functions.get(&name).ok_or_else(|| {
            let detail = if world.functions.contains_key(&name) {
                "function without a body"
            } else {
                "no such function in the checked world"
            };
            VerifiedError::unsupported(Unsupported::ExternalFunction, &name, detail)
        })?;
        // Monomorphization substituted every type parameter of a reachable
        // instance; one that remains has no concrete instance.
        if f.params
            .iter()
            .map(|p| &p.ty)
            .chain([&f.result])
            .any(|t| super::mono::mentions_generic(world, t))
        {
            return Err(VerifiedError::unsupported(
                Unsupported::Generic,
                &name,
                "a type parameter without a concrete instance",
            ));
        }
        effects_supported(world, f)?;
        let caps = inherited.unwrap_or_else(|| row_caps(world, f));
        // Nothing supplies the entry's capabilities: the process starts with
        // no open region.
        if name == entry && !caps.is_empty() {
            return Err(VerifiedError::unsupported(
                Unsupported::Effect,
                &name,
                format!("region capability on the entry ({})", caps.join(", ")),
            ));
        }
        let params = f
            .params
            .iter()
            .map(|p| Ok((p.pass, shape(world, &p.ty, &name)?)))
            .collect::<Result<Vec<_>, VerifiedError>>()?;
        let writeback = f
            .params
            .iter()
            .zip(&params)
            .map(|(p, (_, s))| {
                if p.projections.is_empty() {
                    return Ok(vec![(0, s.len())]);
                }
                p.projections
                    .iter()
                    .map(|path| {
                        s.range(path).ok_or_else(|| {
                            VerifiedError::Invalid(format!(
                                "projection {}.{{{}}} of {name}",
                                p.name,
                                path.join(".")
                            ))
                        })
                    })
                    .collect()
            })
            .collect::<Result<Vec<_>, VerifiedError>>()?;
        let result = shape(world, &f.result, &name)?;
        for b in &f.blocks {
            for i in &b.instructions {
                if let Some((block_label, _, body)) = region_block(world, &i.op, &name) {
                    // The body sees the enclosing capabilities and its own
                    // (which shadows an outer one of the same label).
                    let mut inner: Vec<String> = caps
                        .iter()
                        .filter(|c| **c != block_label)
                        .cloned()
                        .collect();
                    inner.push(block_label);
                    if seen.insert(body.clone()) {
                        queue.push_back((body, Some(inner)));
                    }
                    continue;
                }
                if let Op::Call {
                    callee, capability, ..
                } = &i.op
                    && capability.is_none()
                    && crate::numeric::wrapping_intrinsic(callee).is_none()
                    && seen.insert(callee.clone())
                {
                    queue.push_back((callee.clone(), None));
                }
            }
        }
        index.insert(name.clone(), functions.len());
        functions.push(Planned {
            name,
            label,
            params,
            writeback,
            result,
            caps,
        });
        // Entry label and body label (after the prologue).
        label += 2;
    }
    Ok(Plan { functions, index })
}

/// The lowered body of one function.
pub struct Lowered {
    pub rows: Vec<Lir>,
    /// Slot words, outgoing-area words.
    pub slots: u32,
    pub out_words: u32,
}

struct FnLower<'a> {
    world: &'a World,
    program: &'a Program,
    plan: &'a Plan,
    me: &'a Planned,
    f: &'a FunctionIr,
    rows: Vec<Lir>,
    next_slot: Slot,
    values: BTreeMap<ValueId, (Slot, Shape)>,
    types: BTreeMap<ValueId, Type>,
    /// The block being lowered.
    block: usize,
    /// Arm blocks of a `match` terminator, with the scrutinee: their leading
    /// `Bind` of that value was already tested by the dispatch.
    arm_values: BTreeMap<usize, ValueId>,
    locals: BTreeMap<String, (Slot, Shape)>,
    /// The pointer word of each region capability in scope (region header).
    caps: BTreeMap<String, Slot>,
    block_labels: Vec<Label>,
    out_words: u32,
    next_label: &'a mut Label,
    max_rows: usize,
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::U8 => "u8",
        Kind::U32 => "u32",
        Kind::U64 => "u64",
        Kind::I64 => "i64",
        Kind::Bool => "bool",
        Kind::F64 => "f64",
    }
}

fn kind_of(shape: &Shape, function: &str, what: &str) -> Result<Kind, VerifiedError> {
    match shape.0.as_slice() {
        [(_, k)] => Ok(*k),
        _ => Err(VerifiedError::unsupported(
            Unsupported::Operation,
            function,
            format!("{what} on a non-scalar value"),
        )),
    }
}

impl FnLower<'_> {
    fn err(&self, construct: Unsupported, detail: impl Into<String>) -> VerifiedError {
        VerifiedError::unsupported(construct, &self.f.name, detail)
    }
    fn push(&mut self, row: Lir) -> Result<(), VerifiedError> {
        if self.rows.len() >= self.max_rows {
            return Err(VerifiedError::Limit(format!(
                "more than {} slot-IR rows",
                self.max_rows
            )));
        }
        self.rows.push(row);
        Ok(())
    }
    fn alloc(&mut self, words: u32) -> Result<Slot, VerifiedError> {
        let s = self.next_slot;
        self.next_slot = self
            .next_slot
            .checked_add(words)
            .filter(|n| *n <= super::MAX_FRAME_WORDS)
            .ok_or_else(|| {
                VerifiedError::Limit(format!(
                    "frame of '{}' exceeds {} words",
                    self.f.name,
                    super::MAX_FRAME_WORDS
                ))
            })?;
        Ok(s)
    }
    fn value_of(&mut self, v: ValueId, ty: &Type) -> Result<(Slot, Shape), VerifiedError> {
        if let Some(x) = self.values.get(&v) {
            return Ok(x.clone());
        }
        let shape = shape(self.world, ty, &self.f.name)?;
        let slot = self.alloc(shape.len())?;
        self.values.insert(v, (slot, shape.clone()));
        self.types.insert(v, crate::numeric::default_type(ty));
        Ok((slot, shape))
    }
    fn value(&self, v: ValueId) -> Result<(Slot, Shape), VerifiedError> {
        self.values.get(&v).cloned().ok_or_else(|| {
            VerifiedError::Invalid(format!(
                "value %{v} used before definition in '{}'",
                self.f.name
            ))
        })
    }
    fn local(&mut self, name: &str) -> Result<(Slot, Shape), VerifiedError> {
        if let Some(x) = self.locals.get(name) {
            return Ok(x.clone());
        }
        let ty = self.f.locals.get(name).cloned().ok_or_else(|| {
            VerifiedError::Invalid(format!("unknown local '{name}' in '{}'", self.f.name))
        })?;
        let shape = shape(self.world, &ty, &self.f.name)?;
        let slot = self.alloc(shape.len())?;
        self.locals.insert(name.to_owned(), (slot, shape.clone()));
        Ok((slot, shape))
    }
    fn copy(&mut self, dst: Slot, src: Slot, words: u32) -> Result<(), VerifiedError> {
        for i in 0..words {
            if dst + i != src + i {
                self.push(Lir::Move {
                    dst: dst + i,
                    src: src + i,
                })?;
            }
        }
        Ok(())
    }
    /// The slot range of `root.path`.
    fn place(&mut self, root: &str, path: &[String]) -> Result<(Slot, Shape), VerifiedError> {
        let (slot, shape) = self.local(root)?;
        let (start, len) = shape.range(path).ok_or_else(|| {
            VerifiedError::Invalid(format!("bad place {root}.{}", path.join(".")))
        })?;
        let sub = Shape(shape.0[start as usize..(start + len) as usize].to_vec());
        Ok((slot + start, sub))
    }

    /// The words of `ty`.
    fn words(&self, ty: &Type) -> Result<u32, VerifiedError> {
        Ok(shape(self.world, ty, &self.f.name)?.len())
    }

    /// The sub-values a non-leaf pattern destructures: `(sub-pattern, word
    /// offset, type)`, plus the enum tag the value must carry, if any.
    #[allow(clippy::type_complexity)]
    fn parts<'p>(
        &self,
        p: &'p Pattern,
        ty: &Type,
    ) -> Result<(Option<u64>, Vec<(&'p Pattern, u32, Type)>), VerifiedError> {
        let ty = crate::numeric::default_type(ty);
        match &p.kind {
            PatternKind::Tuple(ps) => {
                let Type::Tuple(ts) = &ty else {
                    return Err(VerifiedError::Invalid(format!("tuple pattern on {ty}")));
                };
                if ts.len() != ps.len() {
                    return Err(VerifiedError::Invalid("tuple pattern arity".into()));
                }
                let mut offset = 0;
                let mut out = vec![];
                for (p, t) in ps.iter().zip(ts) {
                    out.push((p, offset, t.clone()));
                    offset += self.words(t)?;
                }
                Ok((None, out))
            }
            PatternKind::Constructor { name, fields, .. } => {
                if let Some(layout) = variant_layout(self.world, &ty, name, &self.f.name) {
                    let layout = layout?;
                    let mut out = vec![];
                    for (field, p) in fields {
                        let (_, offset, t) = layout
                            .fields
                            .iter()
                            .find(|(n, _, _)| n == field)
                            .ok_or_else(|| {
                                VerifiedError::Invalid(format!("{name} has no field {field}"))
                            })?;
                        out.push((p, *offset, t.clone()));
                    }
                    return Ok((Some(layout.tag), out));
                }
                let Type::Named(n, args) = &ty else {
                    return Err(VerifiedError::Invalid(format!(
                        "constructor pattern on {ty}"
                    )));
                };
                let adt = self
                    .world
                    .adts
                    .get(n)
                    .filter(|a| !a.is_enum && a.variants.len() == 1)
                    .ok_or_else(|| VerifiedError::Invalid(format!("pattern {name} on {ty}")))?;
                let vars = adt_vars(adt, args);
                let mut offsets = BTreeMap::new();
                let mut offset = 0;
                for f in &adt.variants[0].fields {
                    let t = substitute(&f.ty, &vars);
                    let w = self.words(&t)?;
                    offsets.insert(f.name.clone(), (offset, t));
                    offset += w;
                }
                let mut out = vec![];
                for (field, p) in fields {
                    let (offset, t) = offsets.get(field).cloned().ok_or_else(|| {
                        VerifiedError::Invalid(format!("{name} has no field {field}"))
                    })?;
                    out.push((p, offset, t));
                }
                Ok((None, out))
            }
            _ => Ok((None, vec![])),
        }
    }

    /// Bind the names of `p` to the words of the value of type `ty` at
    /// `src`. A refutable part (an enum variant of a multi-variant enum, a
    /// literal) is accepted only when `tested`: a `match` dispatch already
    /// checked it.
    fn bind(
        &mut self,
        p: &Pattern,
        src: Slot,
        ty: &Type,
        tested: bool,
    ) -> Result<(), VerifiedError> {
        match &p.kind {
            PatternKind::Wild => Ok(()),
            PatternKind::Bind(name) => {
                let words = self.words(ty)?;
                let (dst, s) = self.local(name)?;
                if s.len() != words {
                    return Err(VerifiedError::Invalid(format!(
                        "binding '{name}' has {} words, value has {words}",
                        s.len(),
                    )));
                }
                self.copy(dst, src, words)
            }
            PatternKind::Literal(_) if tested => Ok(()),
            PatternKind::Literal(_) => {
                Err(self.err(Unsupported::Match, "refutable literal pattern in a binding"))
            }
            PatternKind::Tuple(_) | PatternKind::Constructor { .. } => {
                if let PatternKind::Constructor { name, .. } = &p.kind
                    && !tested
                    && let Some((adt, _)) = self.world.constructor(name)
                    && adt.is_enum
                    && adt.variants.len() > 1
                {
                    return Err(self.err(
                        Unsupported::Match,
                        format!("refutable constructor pattern {name} in a binding"),
                    ));
                }
                let (_, parts) = self.parts(p, ty)?;
                for (p, offset, t) in parts {
                    self.bind(p, src + offset, &t, tested)?;
                }
                Ok(())
            }
        }
    }

    /// Emit a test that jumps to `fail` unless the value of type `ty` at
    /// `src` matches `p` (first-match `match` dispatch).
    fn test(
        &mut self,
        p: &Pattern,
        src: Slot,
        ty: &Type,
        fail: Label,
    ) -> Result<(), VerifiedError> {
        let check = |this: &mut Self, kind: Kind, word: u64| -> Result<(), VerifiedError> {
            let tmp = this.alloc(2)?;
            this.push(Lir::Const { dst: tmp, word })?;
            this.push(Lir::Compare {
                cond: Cond::Eq,
                kind,
                dst: tmp + 1,
                a: src,
                b: tmp,
            })?;
            this.push(Lir::BranchZero {
                cond: tmp + 1,
                target: fail,
            })
        };
        match &p.kind {
            PatternKind::Wild | PatternKind::Bind(_) => Ok(()),
            PatternKind::Literal(Literal::Int(n)) => {
                let s = shape(self.world, ty, &self.f.name)?;
                let kind = kind_of(&s, &self.f.name, "integer pattern")?;
                let ik = crate::numeric::IntegerKind::from_type(&crate::numeric::default_type(ty))
                    .ok_or_else(|| VerifiedError::Invalid("integer pattern type".into()))?;
                if kind == Kind::Bool || !ik.fits(*n) {
                    // A literal outside the type never matches.
                    return self.push(Lir::Jump(fail));
                }
                check(self, kind, *n as u64)
            }
            PatternKind::Literal(Literal::Bool(b)) => check(self, Kind::Bool, *b as u64),
            PatternKind::Literal(Literal::Float(_)) => {
                Err(self.err(Unsupported::Float, "float pattern"))
            }
            PatternKind::Literal(Literal::String(_)) => {
                Err(self.err(Unsupported::String, "string pattern"))
            }
            #[allow(unreachable_patterns)]
            PatternKind::Literal(_) => Err(self.err(Unsupported::Match, "literal pattern")),
            PatternKind::Tuple(_) | PatternKind::Constructor { .. } => {
                let (tag, parts) = self.parts(p, ty)?;
                if let Some(tag) = tag {
                    check(self, Kind::U64, tag)?;
                }
                for (p, offset, t) in parts {
                    self.test(p, src + offset, &t, fail)?;
                }
                Ok(())
            }
        }
    }

    fn arith(&self, op: &str) -> Option<ArithOp> {
        Some(match op {
            "+" | "+=" => ArithOp::Add,
            "-" | "-=" => ArithOp::Sub,
            "*" | "*=" => ArithOp::Mul,
            "/" | "/=" => ArithOp::Div,
            "%" | "%=" => ArithOp::Rem,
            _ => return None,
        })
    }

    fn float_op(&self, op: ArithOp) -> Result<FloatOp, VerifiedError> {
        Ok(match op {
            ArithOp::Add => FloatOp::Add,
            ArithOp::Sub => FloatOp::Sub,
            ArithOp::Mul => FloatOp::Mul,
            ArithOp::Div => FloatOp::Div,
            ArithOp::Rem => return Err(self.err(Unsupported::Float, "f64 remainder")),
        })
    }

    fn integer(&self, kind: Kind, what: &str) -> Result<Kind, VerifiedError> {
        if matches!(kind, Kind::Bool | Kind::F64) {
            Err(VerifiedError::Invalid(format!(
                "integer {what} on {} in '{}'",
                kind_name(kind),
                self.f.name
            )))
        } else {
            Ok(kind)
        }
    }

    fn instruction(&mut self, i: &Instruction) -> Result<(), VerifiedError> {
        match &i.op {
            Op::Constant(Literal::Int(n)) => {
                let (dst, shape) = self.value_of(i.dest, &i.ty)?;
                let kind = kind_of(&shape, &self.f.name, "integer literal")?;
                let ik =
                    crate::numeric::IntegerKind::from_type(&crate::numeric::default_type(&i.ty))
                        .ok_or_else(|| VerifiedError::Invalid("integer literal type".into()))?;
                if kind == Kind::Bool || !ik.fits(*n) {
                    return Err(VerifiedError::Invalid(format!(
                        "literal {n} does not fit {}",
                        i.ty
                    )));
                }
                // Two's complement for i64, zero extension for unsigned kinds.
                self.push(Lir::Const {
                    dst,
                    word: *n as u64,
                })
            }
            Op::Constant(Literal::Bool(b)) => {
                let (dst, _) = self.value_of(i.dest, &i.ty)?;
                self.push(Lir::Const {
                    dst,
                    word: *b as u64,
                })
            }
            Op::Constant(Literal::Float(x)) => {
                let (dst, shape) = self.value_of(i.dest, &i.ty)?;
                if kind_of(&shape, &self.f.name, "float literal")? != Kind::F64 {
                    return Err(self.err(Unsupported::Float, format!("float literal of {}", i.ty)));
                }
                self.push(Lir::Const {
                    dst,
                    word: x.to_bits(),
                })
            }
            Op::Constant(Literal::String(_)) => {
                Err(self.err(Unsupported::String, "string literal"))
            }
            Op::Unit => {
                self.value_of(i.dest, &Type::unit())?;
                Ok(())
            }
            Op::Read { root, path, .. } => {
                let (src, shape) = self.place(root, path)?;
                let (dst, dshape) = self.value_of(i.dest, &i.ty)?;
                if dshape.len() != shape.len() {
                    return Err(VerifiedError::Invalid(format!(
                        "read of {root} has {} words, expected {}",
                        shape.len(),
                        dshape.len()
                    )));
                }
                self.copy(dst, src, shape.len())
            }
            Op::Bind { pattern, value } => {
                let (src, _) = self.value(*value)?;
                let ty = self
                    .types
                    .get(value)
                    .cloned()
                    .ok_or_else(|| VerifiedError::Invalid(format!("untyped value %{value}")))?;
                self.value_of(i.dest, &Type::unit())?;
                let tested = self.arm_values.get(&self.block) == Some(value);
                self.bind(pattern, src, &ty, tested)
            }
            Op::Write { place, op, value } => {
                let mut parts = place.split('.');
                let root = parts.next().unwrap_or_default().to_owned();
                let path = parts.map(str::to_owned).collect::<Vec<_>>();
                if root.is_empty() {
                    return Err(VerifiedError::Invalid("write without a place".into()));
                }
                let (dst, shape) = self.place(&root, &path)?;
                let (src, vshape) = self.value(*value)?;
                self.value_of(i.dest, &Type::unit())?;
                if op == "=" {
                    if vshape.len() != shape.len() {
                        return Err(VerifiedError::Invalid("write shape mismatch".into()));
                    }
                    return self.copy(dst, src, shape.len());
                }
                let aop = self
                    .arith(op)
                    .ok_or_else(|| self.err(Unsupported::Operation, format!("assignment {op}")))?;
                let kind = kind_of(&shape, &self.f.name, op)?;
                if kind == Kind::F64 {
                    let op = self.float_op(aop)?;
                    return self.push(Lir::FArith {
                        op,
                        dst,
                        a: dst,
                        b: src,
                    });
                }
                let kind = self.integer(kind, op)?;
                // A failing compound assignment raises before its write.
                self.push(Lir::Arith {
                    op: aop,
                    kind,
                    dst,
                    a: dst,
                    b: src,
                })
            }
            Op::Unary(op, v) => {
                let (a, shape) = self.value(*v)?;
                let kind = kind_of(&shape, &self.f.name, op)?;
                let (dst, _) = self.value_of(i.dest, &i.ty)?;
                match op.as_str() {
                    "!" if kind == Kind::Bool => self.push(Lir::Not { dst, a }),
                    "-" if kind == Kind::F64 => self.push(Lir::FNeg { dst, a }),
                    "-" if kind != Kind::Bool => self.push(Lir::Neg { kind, dst, a }),
                    _ => Err(self.err(Unsupported::Operation, format!("unary {op}"))),
                }
            }
            Op::Binary(op, x, y) => {
                let (a, ashape) = self.value(*x)?;
                let (b, bshape) = self.value(*y)?;
                // One word with an empty path is a scalar; anything else
                // (including a one-field struct) is an aggregate.
                if ashape.len() != 1 || bshape.len() != 1 || !ashape.0[0].0.is_empty() {
                    return Err(self.err(
                        Unsupported::StructuralEquality,
                        format!("operator {op} on aggregates"),
                    ));
                }
                let kind = ashape.0[0].1;
                if bshape.0[0].1 != kind {
                    return Err(VerifiedError::Invalid(format!(
                        "operator {op} on different kinds"
                    )));
                }
                let (dst, _) = self.value_of(i.dest, &i.ty)?;
                if kind == Kind::F64 {
                    if let Some(aop) = self.arith(op).filter(|_| !op.ends_with('=')) {
                        let op = self.float_op(aop)?;
                        return self.push(Lir::FArith { op, dst, a, b });
                    }
                    let cond = match op.as_str() {
                        "==" => Cond::Eq,
                        "!=" => Cond::Ne,
                        "<" => Cond::Lt,
                        "<=" => Cond::Le,
                        ">" => Cond::Gt,
                        ">=" => Cond::Ge,
                        _ => {
                            return Err(
                                self.err(Unsupported::Float, format!("operator {op} on f64"))
                            );
                        }
                    };
                    return self.push(Lir::FCompare { cond, dst, a, b });
                }
                if let Some(aop) = self.arith(op).filter(|_| !op.ends_with('=')) {
                    let kind = self.integer(kind, op)?;
                    return self.push(Lir::Arith {
                        op: aop,
                        kind,
                        dst,
                        a,
                        b,
                    });
                }
                if let Some(w) = crate::numeric::wrapping_intrinsic(op) {
                    let kind = self.integer(kind, op)?;
                    let op = match w {
                        "+" => WrapOp::Add,
                        "-" => WrapOp::Sub,
                        _ => WrapOp::Mul,
                    };
                    return self.push(Lir::Wrapping {
                        op,
                        kind,
                        dst,
                        a,
                        b,
                    });
                }
                let cond = match op.as_str() {
                    "==" => Cond::Eq,
                    "!=" => Cond::Ne,
                    "<" => Cond::Lt,
                    "<=" => Cond::Le,
                    ">" => Cond::Gt,
                    ">=" => Cond::Ge,
                    _ => return Err(self.err(Unsupported::Operation, format!("operator {op}"))),
                };
                if kind == Kind::Bool && !matches!(cond, Cond::Eq | Cond::Ne) {
                    return Err(VerifiedError::Invalid(format!("operator {op} on bool")));
                }
                self.push(Lir::Compare {
                    cond,
                    kind,
                    dst,
                    a,
                    b,
                })
            }
            Op::Tuple(vs) => {
                let (dst, _) = self.value_of(i.dest, &i.ty)?;
                let mut offset = 0;
                for v in vs {
                    let (src, s) = self.value(*v)?;
                    self.copy(dst + offset, src, s.len())?;
                    offset += s.len();
                }
                Ok(())
            }
            Op::Data(name, fields) => {
                let Some((adt, variant)) = self.world.constructor(name) else {
                    return Err(VerifiedError::Invalid(format!(
                        "unknown constructor {name}"
                    )));
                };
                if adt.is_enum {
                    let (dst, shape) = self.value_of(i.dest, &i.ty)?;
                    let ty = self.types[&i.dest].clone();
                    let layout = variant_layout(self.world, &ty, name, &self.f.name).ok_or_else(
                        || VerifiedError::Invalid(format!("enum constructor {name} at {ty}")),
                    )??;
                    self.push(Lir::Const {
                        dst,
                        word: layout.tag,
                    })?;
                    let mut written = 1;
                    for (field, offset, _) in &layout.fields {
                        let Some((_, v)) = fields.iter().find(|(n, _)| n == field) else {
                            return Err(VerifiedError::Invalid(format!(
                                "constructor {name} misses field {field}"
                            )));
                        };
                        let (src, s) = self.value(*v)?;
                        self.copy(dst + offset, src, s.len())?;
                        written = written.max(offset + s.len());
                    }
                    // Unused payload words are zero, so results are
                    // deterministic.
                    for w in written..shape.len() {
                        self.push(Lir::Const {
                            dst: dst + w,
                            word: 0,
                        })?;
                    }
                    return Ok(());
                }
                let (dst, shape) = self.value_of(i.dest, &i.ty)?;
                for field in &variant.fields {
                    let Some((_, v)) = fields.iter().find(|(n, _)| n == &field.name) else {
                        return Err(VerifiedError::Invalid(format!(
                            "constructor {name} misses field {}",
                            field.name
                        )));
                    };
                    let (start, len) = shape
                        .range(std::slice::from_ref(&field.name))
                        .ok_or_else(|| VerifiedError::Invalid("struct layout".into()))?;
                    let (src, s) = self.value(*v)?;
                    if s.len() != len {
                        return Err(VerifiedError::Invalid("field shape mismatch".into()));
                    }
                    self.copy(dst + start, src, len)?;
                }
                Ok(())
            }
            Op::Project(v, field) => {
                let (src, shape) = self.value(*v)?;
                let (start, len) = shape
                    .range(std::slice::from_ref(field))
                    .ok_or_else(|| VerifiedError::Invalid(format!("projection .{field}")))?;
                let (dst, _) = self.value_of(i.dest, &i.ty)?;
                self.copy(dst, src + start, len)
            }
            Op::Call {
                callee,
                args,
                types,
                capability,
            } => {
                if let Some(label) = capability {
                    return self.region_operation(i, label, callee, args);
                }
                if !types.is_empty() {
                    return Err(self.err(Unsupported::Generic, format!("call {callee}[...]")));
                }
                let target = self
                    .plan
                    .index
                    .get(callee)
                    .map(|&n| &self.plan.functions[n])
                    .ok_or_else(|| {
                        self.err(
                            Unsupported::ExternalFunction,
                            format!("call to unplanned function {callee}"),
                        )
                    })?;
                let (label, nres, params, ranges, caps) = (
                    target.label,
                    target.result.len(),
                    target.params.clone(),
                    target.writeback.clone(),
                    target.caps.clone(),
                );
                if params.len() != args.len() {
                    return Err(VerifiedError::Invalid(format!("arity of {callee}")));
                }
                let cap_slots = self.cap_arguments(&caps, None)?;
                let area = nres
                    + params.iter().map(|(_, s)| s.len()).sum::<u32>()
                    + cap_slots.len() as u32;
                self.out_words = self.out_words.max(area);
                let mut index = nres;
                let mut writebacks = vec![];
                for ((arg, (pass, s)), ranges) in args.iter().zip(&params).zip(ranges) {
                    let (src, ashape) = self.value(arg.value)?;
                    if ashape.len() != s.len() {
                        return Err(VerifiedError::Invalid(format!(
                            "argument shape of {callee}"
                        )));
                    }
                    for w in 0..s.len() {
                        self.push(Lir::StoreOut {
                            index: index + w,
                            src: src + w,
                        })?;
                    }
                    if *pass == Pass::Inout {
                        let place = arg.place.clone().ok_or_else(|| {
                            VerifiedError::Invalid("inout argument without a place".into())
                        })?;
                        writebacks.push((place, index, ranges));
                    }
                    index += s.len();
                }
                for src in cap_slots {
                    self.push(Lir::StoreOut { index, src })?;
                    index += 1;
                }
                self.push(Lir::Call { callee: label })?;
                let (dst, dshape) = self.value_of(i.dest, &i.ty)?;
                if dshape.len() != nres {
                    return Err(VerifiedError::Invalid(format!("result shape of {callee}")));
                }
                for w in 0..nres {
                    self.push(Lir::LoadOut {
                        dst: dst + w,
                        index: w,
                    })?;
                }
                // Copy back only the words the callee may write: two
                // `inout` arguments may name disjoint projections of one
                // place (`f(inout p, inout p)` with `p.{x}` and `q.{y}`).
                for (place, index, ranges) in writebacks {
                    let (slot, _) = self.place(&place.root, &place.path)?;
                    for (start, len) in ranges {
                        for w in start..start + len {
                            self.push(Lir::LoadOut {
                                dst: slot + w,
                                index: index + w,
                            })?;
                        }
                    }
                }
                Ok(())
            }
            Op::Index(..) => Err(self.err(Unsupported::Index, "dynamic indexing")),
            Op::Indirect { .. } | Op::Closure { .. } => {
                Err(self.err(Unsupported::FunctionValue, "closures and indirect calls"))
            }
            Op::Try(_) => Err(self.err(Unsupported::Effect, "`?`")),
            Op::Handle { aliases, .. } => {
                if let Some((label, capacity, body)) = region_block(self.world, &i.op, &self.f.name)
                {
                    return self.region_block(i, &label, capacity, &body, aliases);
                }
                Err(self.err(Unsupported::Effect, "effect handlers"))
            }
            #[allow(unreachable_patterns)]
            _ => Err(self.err(
                Unsupported::Operation,
                "an IR operation this backend does not know",
            )),
        }
    }

    /// Pointer words of the region capabilities `caps` for a call, from this
    /// function's capabilities (`own` names a header slot that `label`
    /// resolves to first: the block being opened).
    fn cap_arguments(
        &self,
        caps: &[String],
        own: Option<(&str, Slot)>,
    ) -> Result<Vec<Slot>, VerifiedError> {
        caps.iter()
            .map(|c| match own {
                Some((label, slot)) if label == c => Ok(slot),
                _ => self.caps.get(c).copied().ok_or_else(|| {
                    self.err(
                        Unsupported::Effect,
                        format!("region capability '{c}' is not in scope"),
                    )
                }),
            })
            .collect()
    }

    fn word(&mut self, word: u64) -> Result<Slot, VerifiedError> {
        let dst = self.alloc(1)?;
        self.push(Lir::Const { dst, word })?;
        Ok(dst)
    }
    fn load_ptr(&mut self, ptr: Slot, offset: u32) -> Result<Slot, VerifiedError> {
        let dst = self.alloc(1)?;
        self.push(Lir::LoadPtr { dst, ptr, offset })?;
        Ok(dst)
    }
    fn add_u64(&mut self, a: Slot, b: Slot) -> Result<Slot, VerifiedError> {
        let dst = self.alloc(1)?;
        self.push(Lir::Arith {
            op: ArithOp::Add,
            kind: Kind::U64,
            dst,
            a,
            b,
        })?;
        Ok(dst)
    }
    /// Abort with [`TrapKind::Region`] unless `a <= b` (unsigned).
    fn region_bound(&mut self, a: Slot, b: Slot) -> Result<(), VerifiedError> {
        let ok = self.alloc(1)?;
        self.push(Lir::Compare {
            cond: Cond::Le,
            kind: Kind::U64,
            dst: ok,
            a,
            b,
        })?;
        self.push(Lir::BranchZero {
            cond: ok,
            target: TrapKind::Region.label(),
        })
    }
    /// Bump-allocate `words` heap words: `(globals pointer, first word,
    /// words in use before, words in use after)`.
    fn heap_alloc(&mut self, words: u32) -> Result<(Slot, Slot, Slot, Slot), VerifiedError> {
        let globals = self.word(0)?;
        let top = self.load_ptr(globals, G_TOP)?;
        let base = self.word(u64::from(GLOBALS))?;
        let first = self.add_u64(top, base)?;
        let n = self.word(u64::from(words))?;
        let after = self.add_u64(top, n)?;
        let limit = self.word(super::HEAP_WORDS - u64::from(GLOBALS))?;
        self.region_bound(after, limit)?;
        self.push(Lir::StorePtr {
            ptr: globals,
            offset: G_TOP,
            src: after,
        })?;
        Ok((globals, first, top, after))
    }

    /// `region label(capacity) { body }`: open a header, run the body's
    /// nested function with the captured places and capabilities, close.
    fn region_block(
        &mut self,
        i: &Instruction,
        label: &str,
        capacity: usize,
        body: &str,
        aliases: &BTreeMap<String, String>,
    ) -> Result<(), VerifiedError> {
        let target = self
            .plan
            .index
            .get(body)
            .map(|&n| &self.plan.functions[n])
            .ok_or_else(|| VerifiedError::Invalid(format!("unplanned region body {body}")))?;
        let (callee, nres, params, caps) = (
            target.label,
            target.result.len(),
            target.params.clone(),
            target.caps.clone(),
        );
        let f = self
            .program
            .functions
            .get(body)
            .ok_or_else(|| VerifiedError::Invalid(format!("missing region body {body}")))?;
        // Captured places are copied in (and `inout` ones back); a body that
        // assigns a place captured by value would lose the write.
        let by_value: BTreeSet<&str> = f
            .params
            .iter()
            .filter(|p| p.pass != Pass::Inout)
            .map(|p| p.name.as_str())
            .collect();
        for b in &f.blocks {
            for ins in &b.instructions {
                let written: Vec<String> = match &ins.op {
                    Op::Write { place, .. } => place
                        .split('.')
                        .next()
                        .map(str::to_owned)
                        .into_iter()
                        .collect(),
                    Op::Call { args, .. } => args
                        .iter()
                        .filter(|a| a.pass == Pass::Inout)
                        .filter_map(|a| a.place.as_ref().map(|p| p.root.clone()))
                        .collect(),
                    _ => vec![],
                };
                if let Some(root) = written.iter().find(|r| by_value.contains(r.as_str())) {
                    return Err(self.err(
                        Unsupported::Effect,
                        format!("region block assigns the captured place '{root}'"),
                    ));
                }
            }
        }
        // Open: a header in the heap.
        let (globals, header, top, _) = self.heap_alloc(HEADER)?;
        let zero = self.word(0)?;
        let cap = self.word(capacity as u64)?;
        let prev = self.load_ptr(globals, G_INNER)?;
        for (offset, src) in [
            (H_USED, zero),
            (H_ALLOCS, zero),
            (H_PREV, prev),
            (H_START, top),
            (H_CAP, cap),
        ] {
            self.push(Lir::StorePtr {
                ptr: header,
                offset,
                src,
            })?;
        }
        self.push(Lir::StorePtr {
            ptr: globals,
            offset: G_INNER,
            src: header,
        })?;
        // The body, called like a function.
        let cap_slots = self.cap_arguments(&caps, Some((label, header)))?;
        let area = nres + params.iter().map(|(_, s)| s.len()).sum::<u32>() + cap_slots.len() as u32;
        self.out_words = self.out_words.max(area);
        let mut index = nres;
        let mut writebacks = vec![];
        for (p, (pass, s)) in f.params.iter().zip(&params) {
            let root = aliases
                .get(&p.name)
                .cloned()
                .unwrap_or_else(|| p.name.clone());
            let (src, shape) = self.local(&root)?;
            if shape.len() != s.len() {
                return Err(VerifiedError::Invalid(format!(
                    "captured '{}' of {body}",
                    p.name
                )));
            }
            for w in 0..s.len() {
                self.push(Lir::StoreOut {
                    index: index + w,
                    src: src + w,
                })?;
            }
            if *pass == Pass::Inout {
                writebacks.push((src, index, s.len()));
            }
            index += s.len();
        }
        for src in cap_slots {
            self.push(Lir::StoreOut { index, src })?;
            index += 1;
        }
        self.push(Lir::Call { callee })?;
        let (dst, dshape) = self.value_of(i.dest, &i.ty)?;
        if dshape.len() != nres {
            return Err(VerifiedError::Invalid(format!("result shape of {body}")));
        }
        for w in 0..nres {
            self.push(Lir::LoadOut {
                dst: dst + w,
                index: w,
            })?;
        }
        for (slot, index, len) in writebacks {
            for w in 0..len {
                self.push(Lir::LoadOut {
                    dst: slot + w,
                    index: index + w,
                })?;
            }
        }
        // Close: the enclosing block is innermost again, and the heap words
        // above the block's start are free unless an outer region allocated
        // while it was open (`pin`).
        let globals = self.word(0)?;
        let prev = self.load_ptr(header, H_PREV)?;
        self.push(Lir::StorePtr {
            ptr: globals,
            offset: G_INNER,
            src: prev,
        })?;
        let start = self.load_ptr(header, H_START)?;
        let pin = self.load_ptr(globals, G_PIN)?;
        let keep = self.alloc(1)?;
        self.push(Lir::Compare {
            cond: Cond::Gt,
            kind: Kind::U64,
            dst: keep,
            a: pin,
            b: start,
        })?;
        let use_start = *self.next_label;
        let done = *self.next_label + 1;
        *self.next_label += 2;
        self.push(Lir::BranchZero {
            cond: keep,
            target: use_start,
        })?;
        self.push(Lir::StorePtr {
            ptr: globals,
            offset: G_TOP,
            src: pin,
        })?;
        self.push(Lir::Jump(done))?;
        self.push(Lir::Label(use_start))?;
        self.push(Lir::StorePtr {
            ptr: globals,
            offset: G_TOP,
            src: start,
        })?;
        self.push(Lir::Label(done))
    }

    /// `label.op(args)` on a region capability.
    fn region_operation(
        &mut self,
        i: &Instruction,
        label: &str,
        callee: &str,
        args: &[crate::ir::Argument],
    ) -> Result<(), VerifiedError> {
        let op = callee
            .strip_prefix(crate::regions::REGION_EFFECT)
            .and_then(|o| o.strip_prefix("::"))
            .filter(|_| crate::regions::active(self.world))
            .ok_or_else(|| self.err(Unsupported::Effect, format!("effect operation {callee}")))?;
        let header = *self.caps.get(label).ok_or_else(|| {
            self.err(
                Unsupported::Effect,
                format!("region capability '{label}' is not in scope"),
            )
        })?;
        let arg = |n: usize| {
            args.get(n)
                .map(|a| a.value)
                .ok_or_else(|| VerifiedError::Invalid(format!("{callee} lacks an argument")))
        };
        match op {
            "box" => {
                let (src, shape) = self.value(arg(0)?)?;
                let ty = self
                    .types
                    .get(&arg(0)?)
                    .cloned()
                    .ok_or_else(|| VerifiedError::Invalid("untyped region payload".into()))?;
                let size = crate::layout::Layout::of(self.world, &ty)
                    .map_err(|e| self.err(Unsupported::Type, format!("region payload {ty}: {e}")))?
                    .size;
                let bytes = crate::regions::store::slot_bytes(size as usize) as u64;
                if shape.len() > MAX_PAYLOAD_WORDS {
                    return Err(VerifiedError::Limit(format!(
                        "region payload of {} words in '{}'",
                        shape.len(),
                        self.f.name
                    )));
                }
                // `vesper.region.accounting/1`: the bytes, then the count.
                let used = self.load_ptr(header, H_USED)?;
                let b = self.word(bytes)?;
                let used2 = self.add_u64(used, b)?;
                let cap = self.load_ptr(header, H_CAP)?;
                self.region_bound(used2, cap)?;
                let allocs = self.load_ptr(header, H_ALLOCS)?;
                let one = self.word(1)?;
                let allocs2 = self.add_u64(allocs, one)?;
                let max = self.word(crate::regions::store::MAX_ALLOCATIONS as u64)?;
                self.region_bound(allocs2, max)?;
                self.push(Lir::StorePtr {
                    ptr: header,
                    offset: H_USED,
                    src: used2,
                })?;
                self.push(Lir::StorePtr {
                    ptr: header,
                    offset: H_ALLOCS,
                    src: allocs2,
                })?;
                // A zero-word payload still takes one word, so that distinct
                // boxes get distinct handles.
                let (globals, first, _, after) = self.heap_alloc(shape.len().max(1))?;
                // An allocation in a region that is not the innermost pins
                // the heap words below it until the outer region closes.
                let inner = self.load_ptr(globals, G_INNER)?;
                let outer = self.alloc(1)?;
                self.push(Lir::Compare {
                    cond: Cond::Ne,
                    kind: Kind::U64,
                    dst: outer,
                    a: inner,
                    b: header,
                })?;
                let skip = *self.next_label;
                *self.next_label += 1;
                self.push(Lir::BranchZero {
                    cond: outer,
                    target: skip,
                })?;
                self.push(Lir::StorePtr {
                    ptr: globals,
                    offset: G_PIN,
                    src: after,
                })?;
                self.push(Lir::Label(skip))?;
                for w in 0..shape.len() {
                    self.push(Lir::StorePtr {
                        ptr: first,
                        offset: w,
                        src: src + w,
                    })?;
                }
                let (dst, _) = self.value_of(i.dest, &i.ty)?;
                self.push(Lir::Move { dst, src: first })
            }
            "get" => {
                let (ptr, _) = self.value(arg(0)?)?;
                let (dst, shape) = self.value_of(i.dest, &i.ty)?;
                for w in 0..shape.len() {
                    self.push(Lir::LoadPtr {
                        dst: dst + w,
                        ptr,
                        offset: w,
                    })?;
                }
                Ok(())
            }
            "set" => {
                let (ptr, _) = self.value(arg(0)?)?;
                let (src, shape) = self.value(arg(1)?)?;
                self.value_of(i.dest, &Type::unit())?;
                for w in 0..shape.len() {
                    self.push(Lir::StorePtr {
                        ptr,
                        offset: w,
                        src: src + w,
                    })?;
                }
                Ok(())
            }
            "used" => {
                let (dst, _) = self.value_of(i.dest, &i.ty)?;
                self.push(Lir::LoadPtr {
                    dst,
                    ptr: header,
                    offset: H_USED,
                })
            }
            other => Err(self.err(
                Unsupported::Effect,
                format!("region operation '{other}' (only box, get, set and used)"),
            )),
        }
    }

    /// Whether `block` ends in a self call whose result is returned directly.
    fn tail_self_call(&self, block: &Block) -> bool {
        let Some(last) = block.instructions.last() else {
            return false;
        };
        let Op::Call { callee, .. } = &last.op else {
            return false;
        };
        if callee != &self.f.name || self.me.params.iter().any(|(p, _)| *p == Pass::Inout) {
            return false;
        }
        match &block.terminator {
            Terminator::Return(v) => *v == last.dest,
            Terminator::Jump {
                block: target,
                args,
            } => {
                let t = &self.f.blocks[*target];
                args.as_slice() == [last.dest]
                    && t.instructions.is_empty()
                    && t.params.len() == 1
                    && matches!(t.terminator, Terminator::Return(v) if v == t.params[0])
            }
            _ => false,
        }
    }

    fn function(mut self) -> Result<Lowered, VerifiedError> {
        let nres = self.me.result.len();
        // Give every block parameter and instruction result its words up
        // front, so the block order does not matter.
        for block in &self.f.blocks {
            for p in &block.params {
                let ty = self.f.values.get(p).cloned().ok_or_else(|| {
                    VerifiedError::Invalid(format!("untyped block parameter %{p}"))
                })?;
                self.value_of(*p, &ty)?;
            }
            for i in &block.instructions {
                let ty = if matches!(i.op, Op::Bind { .. } | Op::Write { .. } | Op::Unit) {
                    Type::unit()
                } else {
                    i.ty.clone()
                };
                self.value_of(i.dest, &ty)?;
            }
        }
        self.push(Lir::Label(self.me.label))?;
        let enter_at = self.rows.len();
        self.push(Lir::Enter { words: 0 })?;
        self.push(Lir::Label(self.me.label + 1))?;
        // Parameters: incoming area words after the results.
        let mut index = nres;
        let params = self.f.params.clone();
        for p in &params {
            let (slot, s) = self.local(&p.name)?;
            for w in 0..s.len() {
                self.push(Lir::LoadIn {
                    dst: slot + w,
                    index: index + w,
                })?;
            }
            index += s.len();
        }
        for label in self.me.caps.clone() {
            let slot = self.alloc(1)?;
            self.push(Lir::LoadIn { dst: slot, index })?;
            self.caps.insert(label, slot);
            index += 1;
        }
        for block in &self.f.blocks {
            if let Terminator::Match { value, arms } = &block.terminator {
                for (_, target) in arms {
                    self.arm_values.insert(*target, *value);
                }
            }
        }
        for (b, block) in self.f.blocks.iter().enumerate() {
            self.block = b;
            if b > 0 {
                self.push(Lir::Label(self.block_labels[b]))?;
            }
            let tail = self.tail_self_call(block);
            let count = block.instructions.len();
            for (n, i) in block.instructions.iter().enumerate() {
                if tail && n + 1 == count {
                    // A self call in tail position is a loop: write the new
                    // arguments into the incoming area and jump to the body.
                    let Op::Call { args, .. } = &i.op else {
                        unreachable!()
                    };
                    let mut index = nres;
                    let mut staged = vec![];
                    for arg in args {
                        let (src, s) = self.value(arg.value)?;
                        staged.push((src, s.len(), index));
                        index += s.len();
                    }
                    for (src, len, index) in staged {
                        for w in 0..len {
                            self.push(Lir::StoreIn {
                                index: index + w,
                                src: src + w,
                            })?;
                        }
                    }
                    self.push(Lir::Jump(self.me.label + 1))?;
                    continue;
                }
                self.instruction(i)?;
            }
            if tail {
                continue;
            }
            match &block.terminator {
                Terminator::Return(v) => {
                    let (src, s) = self.value(*v)?;
                    if s.len() != nres {
                        return Err(VerifiedError::Invalid(format!(
                            "return shape of '{}'",
                            self.f.name
                        )));
                    }
                    for w in 0..nres {
                        self.push(Lir::StoreIn {
                            index: w,
                            src: src + w,
                        })?;
                    }
                    // `inout` parameters are written back on return.
                    let mut index = nres;
                    for (p, (pass, s)) in params.iter().zip(&self.me.params) {
                        if *pass == Pass::Inout {
                            let (slot, _) = self.local(&p.name)?;
                            for w in 0..s.len() {
                                self.push(Lir::StoreIn {
                                    index: index + w,
                                    src: slot + w,
                                })?;
                            }
                        }
                        index += s.len();
                    }
                    self.push(Lir::Return)?;
                }
                Terminator::Jump { block, args } => {
                    let target = &self.f.blocks[*block];
                    if target.params.len() != args.len() {
                        return Err(VerifiedError::Invalid("block arity".into()));
                    }
                    let mut moves = vec![];
                    for (param, arg) in target.params.iter().zip(args) {
                        let ty = self.f.values.get(param).cloned().ok_or_else(|| {
                            VerifiedError::Invalid(format!("untyped block parameter %{param}"))
                        })?;
                        let (dst, s) = self.value_of(*param, &ty)?;
                        let (src, a) = self.value(*arg)?;
                        if a.len() != s.len() {
                            return Err(VerifiedError::Invalid("jump argument shape".into()));
                        }
                        moves.push((dst, src, s.len()));
                    }
                    for (dst, src, len) in moves {
                        self.copy(dst, src, len)?;
                    }
                    self.push(Lir::Jump(self.block_labels[*block]))?;
                }
                Terminator::Branch { condition, yes, no } => {
                    let (c, s) = self.value(*condition)?;
                    if kind_of(&s, &self.f.name, "branch")? != Kind::Bool {
                        return Err(VerifiedError::Invalid("branch on a non-bool".into()));
                    }
                    self.push(Lir::BranchZero {
                        cond: c,
                        target: self.block_labels[*no],
                    })?;
                    self.push(Lir::Jump(self.block_labels[*yes]))?;
                }
                Terminator::Match { value, arms } => {
                    let (src, _) = self.value(*value)?;
                    let ty =
                        self.types.get(value).cloned().ok_or_else(|| {
                            VerifiedError::Invalid(format!("untyped value %{value}"))
                        })?;
                    for (p, target) in arms {
                        let next = *self.next_label;
                        *self.next_label += 1;
                        self.test(p, src, &ty, next)?;
                        self.push(Lir::Jump(self.block_labels[*target]))?;
                        self.push(Lir::Label(next))?;
                    }
                    // The checker proves the arms exhaustive.
                    self.push(Lir::Trap(TrapKind::Unreachable))?;
                }
                Terminator::Unreachable => self.push(Lir::Trap(TrapKind::Unreachable))?,
            }
        }
        let words = self
            .next_slot
            .checked_add(self.out_words)
            .filter(|n| *n <= super::MAX_FRAME_WORDS)
            .ok_or_else(|| VerifiedError::Limit(format!("frame of '{}'", self.f.name)))?;
        // Keep the stack 16-byte aligned.
        let words = words + (words & 1);
        self.rows[enter_at] = Lir::Enter { words };
        // Slots are addressed above the outgoing area.
        let out = self.out_words;
        for row in &mut self.rows {
            relocate(row, out);
        }
        Ok(Lowered {
            rows: self.rows,
            slots: self.next_slot,
            out_words: out,
        })
    }
}

/// Slot `s` lives at frame word `out + s` (the outgoing area is at the bottom
/// of the frame).
fn relocate(row: &mut Lir, out: u32) {
    match row {
        Lir::Const { dst, .. } | Lir::LoadOut { dst, .. } | Lir::LoadIn { dst, .. } => *dst += out,
        Lir::Move { dst, src } => {
            *dst += out;
            *src += out;
        }
        Lir::Arith { dst, a, b, .. }
        | Lir::Wrapping { dst, a, b, .. }
        | Lir::Compare { dst, a, b, .. } => {
            *dst += out;
            *a += out;
            *b += out;
        }
        Lir::FArith { dst, a, b, .. } | Lir::FCompare { dst, a, b, .. } => {
            *dst += out;
            *a += out;
            *b += out;
        }
        Lir::Neg { dst, a, .. } | Lir::Not { dst, a } | Lir::FNeg { dst, a } => {
            *dst += out;
            *a += out;
        }
        Lir::StoreOut { src, .. } | Lir::StoreIn { src, .. } => *src += out,
        Lir::LoadPtr { dst, ptr, .. } => {
            *dst += out;
            *ptr += out;
        }
        Lir::StorePtr { ptr, src, .. } => {
            *ptr += out;
            *src += out;
        }
        Lir::BranchZero { cond, .. } => *cond += out,
        Lir::Call { .. }
        | Lir::Label(_)
        | Lir::Jump(_)
        | Lir::Enter { .. }
        | Lir::Return
        | Lir::Trap(_) => {}
    }
}

/// Lower one planned function to slot IR.
pub fn function(
    world: &World,
    program: &Program,
    plan: &Plan,
    index: usize,
    next_label: &mut Label,
    max_rows: usize,
) -> Result<Lowered, VerifiedError> {
    let me = &plan.functions[index];
    let f = &program.functions[&me.name];
    let block_labels = (0..f.blocks.len())
        .map(|_| {
            let l = *next_label;
            *next_label += 1;
            l
        })
        .collect();
    FnLower {
        world,
        program,
        plan,
        me,
        f,
        rows: vec![],
        next_slot: 0,
        values: BTreeMap::new(),
        types: BTreeMap::new(),
        block: 0,
        arm_values: BTreeMap::new(),
        locals: BTreeMap::new(),
        caps: BTreeMap::new(),
        block_labels,
        out_words: 0,
        next_label,
        max_rows,
    }
    .function()
}
