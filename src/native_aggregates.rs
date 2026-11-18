//! Aggregates built where they end up (port gaps lane 7, #990).
//!
//! On the native optimize path a constructed aggregate (`Op::Data`,
//! `Op::Tuple`) or an aggregate call result is a pointer to a stack slot.
//! Without a plan each one is built in its own slot and copied word by word
//! into the next: a variant payload into its variant, the variant into the
//! function's result. [`plan`] picks, for each such value that is used
//! exactly once, the place its single use copies it into:
//!
//! - [`Home::Result`]: the value is the block's `return` (or an `Op::Exit` of
//!   the graph itself), so it is built in the result buffer;
//! - [`Home::Field`]: the value is a field of a later constructor in the same
//!   block, so it is built inside that constructor's storage.
//!
//! Constructors of constants then become stores of constants straight into
//! the result (BBR's `outer_new`), and a variant whose payload is a struct
//! literal writes the struct's words into the result once (Ignis's `check`).
//!
//! Soundness: a home is chosen only inside one block, and only for a value
//! with exactly one use, so nothing else can read the value between its
//! construction and its consumer. The result buffer is the caller's fresh
//! output (or the function's own register-result buffer), and a field home
//! belongs to a constructor that does not exist yet, so no operand of the
//! instructions in between can alias it. Graphs that mention `Box` keep the
//! old lowering (FIP reuse slots and drop elaboration track their slots).
use super::*;
use crate::ir::{FunctionIr, Op, Terminator, ValueId};

/// Where an aggregate value of a graph is built ([`plan`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Home {
    /// The graph's result buffer (`sret`).
    Result,
    /// The field at byte `offset` of the constructor that defines value
    /// `parent`, of type `parent_ty`.
    Field {
        parent: ValueId,
        offset: i32,
        parent_ty: Type,
    },
}

/// The homes of the aggregate values of graph `f`: each value of a
/// constructor or a direct call whose single use, later in its own block,
/// copies it into the graph's result or into a field of a constructor.
pub(super) fn plan(
    world: &World,
    f: &FunctionIr,
    generics: &BTreeMap<String, Type>,
) -> BTreeMap<ValueId, Home> {
    let mut homes = BTreeMap::new();
    if f.values
        .values()
        .chain(f.locals.values())
        .any(|t| crate::fip::mentions_box(world, t))
    {
        return homes;
    }
    let ty = |t: &Type| crate::numeric::default_type(&substitute(t, generics));
    let mut uses: BTreeMap<ValueId, usize> = BTreeMap::new();
    for block in &f.blocks {
        for inst in &block.instructions {
            for id in crate::ir_inline::operands(&inst.op) {
                *uses.entry(id).or_default() += 1;
            }
        }
        for id in crate::ir_inline::terminator_operands(&block.terminator) {
            *uses.entry(id).or_default() += 1;
        }
    }
    let result = ty(&f.result);
    for block in &f.blocks {
        for (at, inst) in block.instructions.iter().enumerate() {
            let candidate = match &inst.op {
                Op::Data(..) | Op::Tuple(_) => true,
                Op::Call { callee, .. } => world.functions.contains_key(callee),
                _ => false,
            };
            let own = ty(&inst.ty);
            if !candidate
                || uses.get(&inst.dest) != Some(&1)
                || !Layout::of(world, &own).is_ok_and(|l| l.aggregate())
            {
                continue;
            }
            let consumer = block.instructions[at + 1..]
                .iter()
                .find(|later| crate::ir_inline::operands(&later.op).contains(&inst.dest));
            let home = match consumer.map(|later| (later, &later.op)) {
                Some((later, Op::Data(name, fields))) => fields
                    .iter()
                    .find(|(_, id)| *id == inst.dest)
                    .map(|(n, _)| n.clone())
                    .filter(|n| {
                        field_type(world, &ty(&later.ty), Some(name), n).as_ref() == Some(&own)
                    })
                    .and_then(|n| field_home(world, later.dest, &ty(&later.ty), Some(name), &n)),
                Some((later, Op::Tuple(ids))) => ids
                    .iter()
                    .position(|id| *id == inst.dest)
                    .map(|i| i.to_string())
                    .filter(|n| field_type(world, &ty(&later.ty), None, n).as_ref() == Some(&own))
                    .and_then(|n| field_home(world, later.dest, &ty(&later.ty), None, &n)),
                Some((_, Op::Exit { value, owner })) => {
                    (*value == inst.dest && *owner == f.name && own == result)
                        .then_some(Home::Result)
                }
                Some(_) => None,
                None => matches!(block.terminator, Terminator::Return(id) if id == inst.dest)
                    .then_some(Home::Result)
                    .filter(|_| own == result),
            };
            if let Some(home) = home {
                homes.insert(inst.dest, home);
            }
        }
    }
    homes
}

/// The type of field `field` of `ty` (of variant `variant` for an enum).
fn field_type(world: &World, ty: &Type, variant: Option<&str>, field: &str) -> Option<Type> {
    let layout = Layout::of(world, ty).ok()?;
    let fields = match (variant, &layout.kind) {
        (Some(name), _) => layout.variant(name)?.1.to_vec(),
        (None, LayoutKind::Product(fs)) => fs.clone(),
        _ => return None,
    };
    fields
        .into_iter()
        .find(|f| f.name == field)
        .map(|f| crate::numeric::default_type(&f.ty))
}

/// The home of field `field` of the constructor `parent` of type `ty`.
fn field_home(
    world: &World,
    parent: ValueId,
    ty: &Type,
    variant: Option<&str>,
    field: &str,
) -> Option<Home> {
    let layout = Layout::of(world, ty).ok()?;
    let fields = match (variant, &layout.kind) {
        (Some(name), _) => layout.variant(name)?.1.to_vec(),
        (None, LayoutKind::Product(fs)) => fs.clone(),
        _ => return None,
    };
    let offset = fields.iter().find(|f| f.name == field)?.offset;
    Some(Home::Field {
        parent,
        offset,
        parent_ty: ty.clone(),
    })
}

/// True when every word of `ty` is plain data: no sequence, string, box,
/// handle, function or `f32` half-word, so storage of it needs no zero.
pub(super) fn plain_data(world: &World, ty: &Type) -> bool {
    fn walk(layout: &Layout) -> bool {
        if crate::strings::is_text(&layout.ty) {
            return false;
        }
        match &layout.kind {
            // An `f32` store writes half its word; keep such storage zeroed.
            LayoutKind::Scalar => {
                (crate::numeric::is_scalar_name(layout.ty.name()) && layout.ty.name() != "f32")
                    || layout.ty == Type::unit()
            }
            LayoutKind::Product(fs) => fs.iter().all(|f| walk(&f.layout)),
            LayoutKind::Enum(vs) => vs.iter().all(|v| v.fields.iter().all(|f| walk(&f.layout))),
        }
    }
    Layout::of(world, ty).is_ok_and(|l| walk(&l))
}

/// The storage chosen for the aggregate values of the graph being lowered:
/// each pointer, and whether the value was built there by its home.
pub(super) type HomePointers = BTreeMap<ValueId, (ir::Value, bool)>;

impl<M: Module> Codegen<'_, '_, M> {
    /// The storage value `id` is built in, when [`plan`] gave it a home that
    /// resolves here: the result buffer `sret`, or its field of a parent
    /// constructor (whose storage is allocated now if it has no home).
    pub(super) fn home_pointer(
        &mut self,
        id: ValueId,
        homes: &BTreeMap<ValueId, Home>,
        pointers: &mut HomePointers,
        sret: Option<ir::Value>,
    ) -> Result<Option<ir::Value>, String> {
        if let Some((ptr, _)) = pointers.get(&id) {
            return Ok(Some(*ptr));
        }
        let ptr = match homes.get(&id) {
            None => return Ok(None),
            Some(Home::Result) => match sret {
                Some(out) => out,
                None => return Ok(None),
            },
            Some(Home::Field {
                parent,
                offset,
                parent_ty,
            }) => {
                let base = match self.home_pointer(*parent, homes, pointers, sret)? {
                    Some(base) => base,
                    None => {
                        let size = Layout::of(self.world, parent_ty)?.size;
                        let base = self.alloc_uninit(size);
                        pointers.insert(*parent, (base, false));
                        base
                    }
                };
                self.offset(base, *offset)
            }
        };
        pointers.insert(id, (ptr, true));
        Ok(Some(ptr))
    }
}

/// True when value `id` was built in its home: its consumer skips the copy.
pub(super) fn placed(pointers: &HomePointers, id: ValueId) -> bool {
    pointers.get(&id).is_some_and(|(_, placed)| *placed)
}
