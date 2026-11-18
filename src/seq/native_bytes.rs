//! Cranelift lowering of the block and fixed-width sequence builtins (§13):
//! `copy_from` and `try_copy_from` (one bounds check, then `memmove`),
//! `fill` (`memset` for bytes, a store loop otherwise) and the `read_*` /
//! `write_*` integer accesses of byte sequences (one bounds check, then one
//! unaligned load or store with a byte swap for big-endian).
use super::*;

impl<M: Module> Codegen<'_, '_, M> {
    /// Lowers `op` when it is a block or fixed-width builtin, else `None`.
    pub(super) fn byte_seq_op(
        &mut self,
        op: SeqOp,
        args: &[ir::Value],
        arg_types: &[Type],
    ) -> Result<Option<ir::Value>, String> {
        if !matches!(op, SeqOp::CopyFrom { .. } | SeqOp::Fill | SeqOp::Wide(_)) {
            return Ok(None);
        }
        let arg = |i: usize| {
            args.get(i)
                .copied()
                .ok_or_else(|| format!("native ChunkSeq::{} lacks an operand", op.name()))
        };
        let ty = arg_types
            .first()
            .ok_or_else(|| format!("native ChunkSeq::{} lacks its receiver", op.name()))?;
        let elem = seq::elem(ty)
            .cloned()
            .ok_or_else(|| format!("native ChunkSeq::{} lacks its element type", op.name()))?;
        let elem = crate::numeric::default_type(&elem);
        let stride = self.seq_stride(&elem)?;
        let (h, start, len) = self.seq_view(arg(0)?, ty);
        Ok(Some(match op {
            SeqOp::CopyFrom { total } => {
                let at = arg(1)?;
                let source = arg_types
                    .get(2)
                    .ok_or("native copy_from lacks its source")?;
                let (sh, sstart, n) = self.seq_view(arg(2)?, source);
                let valid = self.seq_fits(len, at, n);
                self.seq_when(valid, total, |this| {
                    let dst = this.seq_elem_addr(h, start, at, stride);
                    let zero = this.zero();
                    let src = this.seq_elem_addr(sh, sstart, zero, stride);
                    let bytes = this.b.ins().imul_imm(n, stride);
                    this.seq_move(dst, src, bytes);
                    Ok(())
                })?
            }
            SeqOp::Fill => {
                let value = arg(1)?;
                let scalar = !Layout::of(self.world, &elem)?.aggregate();
                if stride == 1 && scalar && self.seq_has_libc() {
                    let zero = self.zero();
                    let dst = self.seq_elem_addr(h, start, zero, 1);
                    let byte = self.coerce(value, Some(&elem));
                    let byte = self.b.ins().ireduce(types::I8, byte);
                    let config = self.m.target_config();
                    self.b.call_memset(config, dst, byte, len);
                } else {
                    let head = self.b.create_block();
                    let body = self.b.create_block();
                    let done = self.b.create_block();
                    self.b.append_block_param(head, types::I64);
                    let zero = self.zero();
                    self.jump(head, &[zero.into()]);
                    self.b.switch_to_block(head);
                    let k = self.b.block_params(head)[0];
                    let more = self.b.ins().icmp(IntCC::UnsignedLessThan, k, len);
                    self.branch(more, body, &[], done, &[]);
                    self.b.switch_to_block(body);
                    let addr = self.seq_elem_addr(h, start, k, stride);
                    self.seq_store_elem(addr, value, &elem)?;
                    let next = self.b.ins().iadd_imm(k, 1);
                    self.jump(head, &[next.into()]);
                    self.b.switch_to_block(done);
                }
                self.zero()
            }
            SeqOp::Wide(wide) => {
                let at = arg(1)?;
                let width = self.b.ins().iconst(types::I64, i64::from(wide.bytes));
                let valid = self.seq_fits(len, at, width);
                let addr = self.seq_elem_addr(h, start, at, 1);
                let int = match wide.bytes {
                    2 => types::I16,
                    4 => types::I32,
                    _ => types::I64,
                };
                if wide.write {
                    let value = arg(2)?;
                    self.seq_when(valid, wide.total, |this| {
                        let mut v = value;
                        if int != types::I64 {
                            v = this.b.ins().ireduce(int, v);
                        }
                        if wide.big {
                            v = this.b.ins().bswap(v);
                        }
                        this.b.ins().store(MemFlags::new(), v, addr, 0);
                        Ok(())
                    })?
                } else {
                    let load = move |this: &mut Self| {
                        let mut v = this.b.ins().load(int, MemFlags::new(), addr, 0);
                        if wide.big {
                            v = this.b.ins().bswap(v);
                        }
                        if int != types::I64 {
                            v = this.b.ins().uextend(types::I64, v);
                        }
                        Ok(v)
                    };
                    if wide.total {
                        let int_ty = Type::named(wide.int_type());
                        let option = Type::Named("Option".into(), vec![int_ty.clone()]);
                        self.seq_option(&option, &int_ty, valid, load)?
                    } else {
                        self.seq_guard(valid)?;
                        load(self)?
                    }
                }
            }
            _ => unreachable!("matched above"),
        }))
    }

    /// Whether `n` elements from `at` fit in `len`, without overflow.
    fn seq_fits(&mut self, len: ir::Value, at: ir::Value, n: ir::Value) -> ir::Value {
        let below = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, at, len);
        let room = self.b.ins().isub(len, at);
        let fits = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, n, room);
        self.b.ins().band(below, fits)
    }

    /// Runs `write` when `valid` holds. The checked form raises
    /// `IndexOutOfBounds` otherwise and yields `Unit`; the total form skips
    /// the write and yields `valid` as a `bool`.
    fn seq_when(
        &mut self,
        valid: ir::Value,
        total: bool,
        write: impl FnOnce(&mut Self) -> Result<(), String>,
    ) -> Result<ir::Value, String> {
        if !total {
            self.seq_guard(valid)?;
            write(self)?;
            return Ok(self.zero());
        }
        let yes = self.b.create_block();
        let done = self.b.create_block();
        self.branch(valid, yes, &[], done, &[]);
        self.b.switch_to_block(yes);
        write(self)?;
        self.jump(done, &[]);
        self.b.switch_to_block(done);
        Ok(self.b.ins().uextend(types::I64, valid))
    }

    /// Whether generated code can call the C library (`memmove`, `memset`):
    /// the JIT and every hosted executable, but not a freestanding image.
    fn seq_has_libc(&self) -> bool {
        std::any::type_name::<M>().contains("JITModule")
            || self.m.isa().triple().operating_system != target_lexicon::OperatingSystem::None_
    }

    /// Copies `bytes` bytes from `src` to `dst`, which may overlap.
    pub(super) fn seq_move(&mut self, dst: ir::Value, src: ir::Value, bytes: ir::Value) {
        if self.seq_has_libc() {
            let config = self.m.target_config();
            self.b.call_memmove(config, dst, src, bytes);
        } else {
            // Only `copy_from` and `extend` move, and their checker keeps the
            // source out of the destination, so a forward copy is exact.
            self.seq_copy(dst, src, bytes);
        }
    }
}
