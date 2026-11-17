//! Cranelift lowering of the operations [`crate::bounds_hoist`] creates: the
//! loop guard [`SeqOp::BoundsGuard`] and the unchecked element accesses
//! [`SeqOp::Hoisted`] of a counted loop's fast copy.
use super::*;
use crate::seq::Access;

impl<M: Module> Codegen<'_, '_, M> {
    /// Lowers `op` when it is a hoisting operation, else `None`.
    pub(super) fn hoisted_seq_op(
        &mut self,
        op: SeqOp,
        args: &[ir::Value],
        arg_types: &[Type],
    ) -> Result<Option<ir::Value>, String> {
        let arg = |i: usize| {
            args.get(i)
                .copied()
                .ok_or_else(|| format!("native ChunkSeq::{} lacks an operand", op.name()))
        };
        match op {
            SeqOp::BoundsGuard { inclusive } => {
                let (lo, hi) = (arg(0)?, arg(1)?);
                let fits = if inclusive {
                    IntCC::UnsignedLessThan
                } else {
                    IntCC::UnsignedLessThanOrEqual
                };
                let empty = if inclusive {
                    IntCC::UnsignedGreaterThan
                } else {
                    IntCC::UnsignedGreaterThanOrEqual
                };
                let mut ok = self.b.ins().icmp(empty, lo, hi);
                let mut inside = None;
                // `offset + hi` fits `len` without overflow: `offset <= len`
                // and `hi` fits `len - offset`.
                for k in (2..arg_types.len()).step_by(2) {
                    let (_, _, len) = self.seq_view(arg(k)?, &arg_types[k]);
                    let offset = arg(k + 1)?;
                    let room = self.b.ins().isub(len, offset);
                    let below = self
                        .b
                        .ins()
                        .icmp(IntCC::UnsignedLessThanOrEqual, offset, len);
                    let c = self.b.ins().icmp(fits, hi, room);
                    let c = self.b.ins().band(below, c);
                    inside = Some(match inside {
                        Some(all) => self.b.ins().band(all, c),
                        None => c,
                    });
                }
                if let Some(inside) = inside {
                    ok = self.b.ins().bor(ok, inside);
                }
                Ok(Some(self.b.ins().uextend(types::I64, ok)))
            }
            SeqOp::Hoisted(access) => {
                let ty = arg_types
                    .first()
                    .ok_or("native hoisted access lacks its receiver")?;
                let elem = seq::elem(ty)
                    .cloned()
                    .ok_or("native hoisted access lacks its element type")?;
                let elem = crate::numeric::default_type(&elem);
                let stride = self.seq_stride(&elem)?;
                let (h, start, _) = self.seq_view(arg(0)?, ty);
                let i = arg(1)?;
                Ok(Some(match access {
                    Access::Get => {
                        let addr = self.seq_elem_addr(h, start, i, stride);
                        self.seq_load_elem(addr, &elem)?
                    }
                    Access::Set | Access::TrySet => {
                        let addr = self.seq_elem_addr(h, start, i, stride);
                        self.seq_store_elem(addr, arg(2)?, &elem)?;
                        if access == Access::Set {
                            self.zero()
                        } else {
                            self.b.ins().iconst(types::I64, 1)
                        }
                    }
                    Access::TryGet => {
                        let option = Type::Named("Option".into(), vec![elem.clone()]);
                        let present = self.b.ins().iconst(types::I8, 1);
                        self.seq_option(&option, &elem, present, |this| {
                            let addr = this.seq_elem_addr(h, start, i, stride);
                            this.seq_load_elem(addr, &elem)
                        })?
                    }
                }))
            }
            _ => Ok(None),
        }
    }
}
