//! Lowering of the `Builtin` operations.

use super::*;

use emela_core::Builtin;

impl<'m> Builder<'m> {
    pub(super) fn builtin(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        op: Builtin,
        args: &[Operand],
    ) {
        use Builtin::*;
        match op {
            StringByteSize => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("str_byte_size");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            StringAppend => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("str_append");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            StringContains => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("str_contains");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            StringStartsWith => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("str_starts_with");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            StringEndsWith => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("str_ends_with");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            StringSplit => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("str_split");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            StringJoin => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("str_join");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            StringTrim => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("str_trim");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            StringCodePoints => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("str_code_points");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            // The builtins that return `Option[A]`: the runtime builds the `Some` from the
            // descriptor and gives back the static `None`.
            IntParse => self.option_call(cx, ins, dst, args, "int_parse"),
            IntCheckedAdd => self.option_call(cx, ins, dst, args, "int_checked_add"),
            StringFromCodePoint => self.option_call(cx, ins, dst, args, "from_code_point"),
            IntToString => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("int_to_string");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            Int64ToString => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("int64_to_string");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            IntBitAnd | Int64BitAnd => self.bit(ins, dst, args, cx, "and"),
            IntBitOr | Int64BitOr => self.bit(ins, dst, args, cx, "or"),
            IntBitXor | Int64BitXor => self.bit(ins, dst, args, cx, "xor"),
            IntBitNot => {
                self.operand(cx, ins, &args[0]);
                ins.i32_const(-1);
                ins.i32_xor();
                self.assign(cx, ins, dst, Repr::I32);
            }
            Int64BitNot => {
                self.operand(cx, ins, &args[0]);
                ins.i64_const(-1);
                ins.i64_xor();
                self.assign(cx, ins, dst, Repr::I64);
            }
            IntShiftLeft => self.shift(cx, ins, dst, args, "ishl"),
            IntShiftRight => self.shift(cx, ins, dst, args, "ishr"),
            IntShiftRightUnsigned => self.shift(cx, ins, dst, args, "iushr"),
            Int64ShiftLeft => self.shift64(cx, ins, dst, args, "lshl"),
            Int64ShiftRight => self.shift64(cx, ins, dst, args, "lshr"),
            Int64ShiftRightUnsigned => self.shift64(cx, ins, dst, args, "lushr"),
            Int64FromInt => {
                self.operand(cx, ins, &args[0]);
                ins.i64_extend_i32_s();
                self.assign(cx, ins, dst, Repr::I64);
            }
            Int64ToInt => {
                self.operand(cx, ins, &args[0]);
                ins.i32_wrap_i64();
                self.assign(cx, ins, dst, Repr::I32);
            }
            FloatFromInt => {
                self.operand(cx, ins, &args[0]);
                ins.f64_convert_i32_s();
                self.assign(cx, ins, dst, Repr::F64);
            }
            CellNew => {
                let Some(d) = dst else { return };
                let content = args
                    .first()
                    .map_or(Repr::Void, |o| Self::operand_repr(cx.f, o));
                let desc = self.desc(DescKey {
                    kind: 0,
                    tag: 0,
                    enum_id: -1,
                    name: "$Cell".to_owned(),
                    show: None,
                    refs: vec![content == Repr::Ref],
                });
                self.alloc(cx, ins, d, desc, 1);
                if let Some(a) = args.first() {
                    self.store_slot(cx, ins, d, 0, a);
                } else {
                    self.store_slot(cx, ins, d, 0, &Operand::Const(Const::Unit));
                }
                // The cell keeps a reference of its own, so the caller can still give up its own
                // (the C runtime dups in `em_cell_new`).
                if content == Repr::Ref {
                    let a = args.first().expect("a reference has a value");
                    self.operand(cx, ins, a);
                    let dup = self.rt("dup");
                    ins.call(dup);
                }
            }
            CellGet => {
                if let Some(d) = dst {
                    let base = match &args[0] {
                        Operand::Var(v) => *v,
                        _ => {
                            self.internal("cell base is not a variable");
                            return;
                        }
                    };
                    ins.local_get(cx.local(base));
                    match cx.f.vars[d].repr {
                        Repr::I32 | Repr::Ref => {
                            ins.i32_load(mem(HDR, 4));
                        }
                        Repr::I64 => {
                            ins.i64_load(mem(HDR, 8));
                        }
                        Repr::F64 => {
                            ins.f64_load(mem(HDR, 8));
                        }
                        Repr::Void => {}
                    }
                    ins.local_set(cx.local(d));
                    // The reader gets a reference of its own (the C runtime dups in
                    // `em_cell_get`), so the caller may give up what it reads.
                    if cx.f.vars[d].repr == Repr::Ref {
                        ins.local_get(cx.local(d));
                        let dup = self.rt("dup");
                        ins.call(dup);
                    }
                }
            }
            CellSet => {
                let base = match &args[0] {
                    Operand::Var(v) => *v,
                    _ => {
                        self.internal("cell base is not a variable");
                        return;
                    }
                };
                match Self::operand_repr(cx.f, &args[1]) {
                    Repr::Ref => {
                        // Take the cell's own reference before replacing the old value, then
                        // release it (the C runtime dups and drops in `em_cell_set`).
                        self.operand(cx, ins, &args[1]);
                        let dup = self.rt("dup");
                        ins.call(dup);
                        let old = cx.scratch(Repr::Ref, 0);
                        ins.local_get(cx.local(base));
                        ins.i32_load(mem(HDR, 4));
                        ins.local_set(old);
                        self.store_slot(cx, ins, base, 0, &args[1]);
                        ins.local_get(old);
                        let drop = self.rt("drop");
                        ins.call(drop);
                    }
                    Repr::Void => {}
                    _ => self.store_slot(cx, ins, base, 0, &args[1]),
                }
            }
            other => {
                self.internal(format!("builtin {other:?} left in {}", cx.f.name));
            }
        }
    }

    /// Calls a runtime builtin that returns `Option[A]`: it takes the arguments, then the `Some`
    /// descriptor and the static `None` object.
    fn option_call(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        args: &[Operand],
        name: &'static str,
    ) {
        let Some(ty) = dst.map(|d| cx.f.vars[d].ty) else {
            self.internal(format!("`{name}` without a destination in {}", cx.f.name));
            return;
        };
        let Some((some, none, _)) = self.option_parts(ty) else {
            self.internal(format!(
                "`{name}` does not return an Option in {}",
                cx.f.name
            ));
            return;
        };
        for a in args {
            self.operand(cx, ins, a);
        }
        ins.i32_const(some as i32);
        ins.i32_const(none as i32);
        let f = self.rt(name);
        ins.call(f);
        self.assign(cx, ins, dst, Repr::Ref);
    }

    fn bit(
        &mut self,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        args: &[Operand],
        cx: &mut FnCtx,
        which: &str,
    ) {
        self.operand(cx, ins, &args[0]);
        self.operand(cx, ins, &args[1]);
        let r = Self::operand_repr(cx.f, &args[0]);
        match (which, r) {
            ("and", Repr::I64) => {
                ins.i64_and();
            }
            ("or", Repr::I64) => {
                ins.i64_or();
            }
            ("xor", Repr::I64) => {
                ins.i64_xor();
            }
            ("and", _) => {
                ins.i32_and();
            }
            ("or", _) => {
                ins.i32_or();
            }
            _ => {
                ins.i32_xor();
            }
        }
        self.assign(cx, ins, dst, r);
    }

    fn shift(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        args: &[Operand],
        name: &'static str,
    ) {
        self.operand(cx, ins, &args[0]);
        self.operand(cx, ins, &args[1]);
        let f = self.rt(name);
        ins.call(f);
        self.assign(cx, ins, dst, Repr::I32);
    }

    fn shift64(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        args: &[Operand],
        name: &'static str,
    ) {
        self.operand(cx, ins, &args[0]);
        self.operand(cx, ins, &args[1]);
        let f = self.rt(name);
        ins.call(f);
        self.assign(cx, ins, dst, Repr::I64);
    }
}
