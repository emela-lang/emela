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
            StringLength => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("str_grapheme_count");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
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
            StringChars => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("str_grapheme_chars");
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
            // `runtime/text/ascii.rs`, a trimmable module: the body is only emitted for a program that
            // changes ASCII case.
            StringAsciiLower | StringAsciiUpper => {
                self.operand(cx, ins, &args[0]);
                let name = match op {
                    StringAsciiLower => "str_ascii_lower",
                    _ => "str_ascii_upper",
                };
                let f = self.rt(name);
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            // The builtins that return `Option[A]`: the runtime builds the `Some` from the
            // descriptor and gives back the static `None`.
            IntParse => self.option_call(cx, ins, dst, args, "int_parse"),
            IntCheckedAdd => self.option_call(cx, ins, dst, args, "int_checked_add"),
            StringFromCodePoint => self.option_call(cx, ins, dst, args, "from_code_point"),
            // A Bytes is a string object whose bytes need not be UTF-8 (`runtime/text/bytes.rs`).
            BytesSize => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("str_byte_size");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
            }
            BytesFromList => self.option_call(cx, ins, dst, args, "bytes_from_list"),
            BytesGet => self.option_call(cx, ins, dst, args, "bytes_get"),
            BytesToList | BytesSlice | BytesConcat => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let name = match op {
                    BytesToList => "bytes_to_list",
                    BytesSlice => "bytes_slice",
                    _ => "bytes_concat",
                };
                let f = self.rt(name);
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            // The same object, owned once more (arguments are borrowed and results owned).
            BytesEncodeUtf8 | BytesDecodeValidUtf8 => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("dup");
                ins.call(f);
                self.operand(cx, ins, &args[0]);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            // Arrays (`runtime/array.rs`). The functions that make an array from lists are given
            // its descriptor, which says whether the elements are references and their stride.
            ArrayFromList | ArrayConcat => {
                let Some(d) = dst else {
                    self.internal(format!("array without a destination in {}", cx.f.name));
                    return;
                };
                let ty = cx.f.vars[d].ty;
                let agg = op == ArrayFromList && self.inline_array_elem(ty).is_some();
                let desc = self.array_desc(ty);
                self.operand(cx, ins, &args[0]);
                ins.i32_const(desc as i32);
                let f = self.rt(match (op, agg) {
                    (ArrayFromList, true) => "array_from_list_agg",
                    (ArrayFromList, false) => "array_from_list",
                    _ => "array_concat",
                });
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            ArrayToList => {
                if dst.is_none() {
                    self.internal(format!("array without a destination in {}", cx.f.name));
                    return;
                }
                // A list head is a reference, so an inline element is boxed per element by
                // `array_to_list_agg`; E2 step 3 does not inline this read.
                let (elem_ref, elem_desc) = match &args[0] {
                    Operand::Var(v) => {
                        let ty = cx.f.vars[*v].ty;
                        (
                            self.array_elem_repr(ty) == Repr::Ref,
                            self.inline_array_elem_desc(ty),
                        )
                    }
                    Operand::Const(_) => (false, None),
                };
                let cons = if elem_ref {
                    self.cons_ref_desc
                } else {
                    self.cons_val_desc
                };
                self.operand(cx, ins, &args[0]);
                ins.i32_const(cons as i32);
                if let Some(elem) = elem_desc {
                    ins.i32_const(elem as i32);
                    let f = self.rt("array_to_list_agg");
                    ins.call(f);
                } else {
                    let f = self.rt("array_to_list");
                    ins.call(f);
                }
                self.assign(cx, ins, dst, Repr::Ref);
            }
            ArraySize => {
                self.operand(cx, ins, &args[0]);
                ins.i32_load(mem(HDR, 4));
                self.assign(cx, ins, dst, Repr::I32);
            }
            ArrayGet => {
                let Some(d) = dst else {
                    self.internal(format!("`ArrayGet` without a destination in {}", cx.f.name));
                    return;
                };
                let array_ty = match &args[0] {
                    Operand::Var(v) => cx.f.vars[*v].ty,
                    Operand::Const(_) => {
                        self.internal(format!("`ArrayGet` of a constant in {}", cx.f.name));
                        return;
                    }
                };
                let Some(elem) = self.inline_array_elem_desc(array_ty) else {
                    self.option_call(cx, ins, dst, args, "array_get");
                    return;
                };
                let Some((some, none, _)) = self.option_parts(cx.f.vars[d].ty) else {
                    self.internal(format!(
                        "`array_get` does not return an Option in {}",
                        cx.f.name
                    ));
                    return;
                };
                // `Option[A]` has two variants, so it has no by-value form: the `Some` cell holds
                // its payload as one reference. Even an inline element is therefore copied into
                // a new box before the cell is made (E2 step 3 leaves this read boxed).
                for a in args {
                    self.operand(cx, ins, a);
                }
                ins.i32_const(elem as i32);
                ins.i32_const(some as i32);
                ins.i32_const(none as i32);
                let f = self.rt("array_get_agg");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            // The Core IR fusion of `Option.or(Array.get(a, i), d)`: the element when `i` is in
            // range, `d` (evaluated before) otherwise. As everywhere, a reference result is owned
            // once more, so it is duped here whether it comes from the array or from `d`.
            ArrayGetOr => {
                let Some(dst) = dst else {
                    self.internal(format!(
                        "`ArrayGetOr` without a destination in {}",
                        cx.f.name
                    ));
                    return;
                };
                let repr = self.repr(cx.f.vars[dst].ty);
                if repr == Repr::Void {
                    self.internal(format!("`ArrayGetOr` of () in {}", cx.f.name));
                    return;
                }
                // A value held in locals is unboxed from the boxed element or read leaf by leaf
                // from the inline element (E1 step 5).
                if cx.aggregate(dst).is_some() {
                    self.array_get_or_value(cx, ins, dst, args);
                    return;
                }
                if let Operand::Var(v) = &args[0]
                    && let Some(elem) = self.inline_array_elem_desc(cx.f.vars[*v].ty)
                {
                    self.operand(cx, ins, &args[0]);
                    self.operand(cx, ins, &args[1]);
                    self.operand(cx, ins, &args[2]);
                    ins.i32_const(elem as i32);
                    let f = self.rt("array_get_or_agg");
                    ins.call(f);
                    self.assign(cx, ins, Some(dst), Repr::Ref);
                    return;
                }
                self.operand(cx, ins, &args[1]);
                ins.i32_const(0);
                ins.i32_ge_s();
                self.operand(cx, ins, &args[1]);
                self.operand(cx, ins, &args[0]);
                ins.i32_load(mem(HDR, 4));
                ins.i32_lt_s();
                ins.i32_and();
                ins.if_(BlockType::Result(val(repr)));
                self.array_element(cx, ins, &args[0], &args[1], repr);
                ins.else_();
                self.operand(cx, ins, &args[2]);
                if repr == Repr::Ref {
                    // `d` is borrowed; the result is owned once more (`dup` consumes its
                    // argument, so keep it in a scratch local).
                    let tmp = cx.scratch(Repr::Ref, 0);
                    ins.local_set(tmp);
                    ins.local_get(tmp);
                    let f = self.rt("dup");
                    ins.call(f);
                    ins.local_get(tmp);
                }
                ins.end();
                self.assign(cx, ins, Some(dst), repr);
            }
            // The Core IR fusion of `match Array.get(a, i)`: the arms' bounds test has picked the
            // element, so there is no check here. It is owned once more.
            ArrayLoadUnchecked => {
                let Some(dst) = dst else {
                    self.internal(format!(
                        "`ArrayLoadUnchecked` without a destination in {}",
                        cx.f.name
                    ));
                    return;
                };
                let repr = self.repr(cx.f.vars[dst].ty);
                if repr == Repr::Void {
                    self.internal(format!("`ArrayLoadUnchecked` of () in {}", cx.f.name));
                    return;
                }
                if cx.aggregate(dst).is_some() {
                    self.array_load_value(cx, ins, dst, args);
                    return;
                }
                if let Operand::Var(v) = &args[0]
                    && let Some(elem) = self.inline_array_elem_desc(cx.f.vars[*v].ty)
                {
                    self.operand(cx, ins, &args[0]);
                    self.operand(cx, ins, &args[1]);
                    ins.i32_const(elem as i32);
                    let f = self.rt("array_load_unchecked_agg");
                    ins.call(f);
                    self.assign(cx, ins, Some(dst), Repr::Ref);
                    return;
                }
                self.array_element(cx, ins, &args[0], &args[1], repr);
                self.assign(cx, ins, Some(dst), repr);
            }
            // The Core IR fusion of `Array.from_list(List.map(List.range(lo, hi), f))`: one array,
            // filled by calling `f` with each position from `lo`. `f` is pure and takes one `Int`,
            // so the indirect call is the closure and that value and returns the element's
            // representation. An element is stored only after `f` returns it, and a defect from
            // `f` ends the loop at the first one, shortens the array to the elements already
            // written and releases it: nothing else releases the destination of a statement that
            // defects, because a destination is not live before its own statement.
            ArrayMapRange => {
                let Some(dst) = dst else {
                    self.internal(format!(
                        "`ArrayMapRange` without a destination in {}",
                        cx.f.name
                    ));
                    return;
                };
                // A type parameter can become `()` after monomorphization: its elements have no
                // representation, so the loop calls `f` for each position and stores nothing.
                let elem = self.array_elem_repr(cx.f.vars[dst].ty);
                let inline = self.inline_array_elem(cx.f.vars[dst].ty);
                let Operand::Var(fun) = &args[2] else {
                    self.internal(format!("a closure that is not a value in {}", cx.f.name));
                    return;
                };
                let fun = cx.local(*fun);
                let out = cx.local(dst);
                let desc = self.array_desc(cx.f.vars[dst].ty);
                // The count goes to `array_new` in 64 bits: `hi - lo` is up to 2^32, which an i32
                // would wrap to a negative count (and so to a tiny array). The position in the
                // array (0-based) is an i32, and the loop reads the count from the array's length.
                let count = cx.scratch(Repr::I64, 0);
                let at = cx.scratch(Repr::I32, 0);
                self.operand(cx, ins, &args[1]);
                ins.i64_extend_i32_s();
                self.operand(cx, ins, &args[0]);
                ins.i64_extend_i32_s();
                ins.i64_sub();
                ins.local_set(count);
                ins.local_get(count);
                ins.i64_const(0);
                ins.i64_lt_s();
                ins.if_(BlockType::Result(ValType::I64));
                ins.i64_const(0);
                ins.else_();
                ins.local_get(count);
                ins.end();
                ins.i32_const(desc as i32);
                let new = self.rt("array_new");
                ins.call(new);
                ins.local_set(out);
                ins.i32_const(0);
                ins.local_set(at);
                ins.block(BlockType::Empty);
                ins.loop_(BlockType::Empty);
                ins.local_get(at);
                ins.local_get(out);
                ins.i32_load(mem(HDR, 4));
                ins.i32_ge_u();
                ins.br_if(1);
                // The address of element `at`.
                if inline.is_none() {
                    ins.local_get(out);
                    ins.i32_const((HDR + 8) as i32);
                    ins.i32_add();
                    ins.local_get(at);
                    ins.i32_const(3);
                    ins.i32_shl();
                    ins.i32_add();
                }
                // `f(lo + at)`: the closure, its argument, then the function's table index.
                let ty = self.call_ty(&[Repr::Ref, Repr::I32], elem);
                ins.local_get(fun);
                self.operand(cx, ins, &args[0]);
                ins.local_get(at);
                ins.i32_add();
                ins.local_get(fun);
                ins.i32_load(mem(HDR, 4));
                ins.call_indirect(0, ty);
                if let Some(layout) = &inline {
                    // The closure returns a box of the element. Keep it while a defect is
                    // checked (a defect returns 0, which must not be dereferenced), copy its
                    // slots into the inline element, and release the box.
                    let slots = layout.slots() as u32;
                    let boxed = cx.scratch(Repr::Ref, 0);
                    ins.local_set(boxed);
                    ins.global_get(G_DEFECT);
                    ins.if_(BlockType::Empty);
                    ins.local_get(out);
                    ins.local_get(at);
                    ins.i32_store(mem(HDR, 4));
                    ins.local_get(out);
                    let drop = self.rt("drop");
                    ins.call(drop);
                    ins.br(2);
                    ins.end();
                    super::runtime::array::elem_at(ins, out, at);
                    ins.local_get(boxed);
                    ins.i32_const(HDR as i32);
                    ins.i32_add();
                    ins.i32_const((slots * 8) as i32);
                    ins.memory_copy(0, 0);
                    ins.local_get(boxed);
                    let drop = self.rt("drop");
                    ins.call(drop);
                } else {
                    match elem {
                        Repr::I32 | Repr::Ref => {
                            ins.i32_store(mem(0, 4));
                        }
                        Repr::I64 => {
                            ins.i64_store(mem(0, 8));
                        }
                        Repr::F64 => {
                            ins.f64_store(mem(0, 8));
                        }
                        Repr::Void => {}
                    }
                    // A defect from `f` (nobody wrote it into the type) ends the loop where it
                    // happens: the elements from `at` on were never written, so the length becomes
                    // the count written. The array is the statement's destination, which a landing
                    // does not own on the defect path (a destination is not live before its own
                    // statement), so the array is released here, and the landing unwinds without it.
                    // `br 2` leaves the `if`, the loop and the block around it.
                    ins.global_get(G_DEFECT);
                    ins.if_(BlockType::Empty);
                    ins.local_get(out);
                    ins.local_get(at);
                    ins.i32_store(mem(HDR, 4));
                    ins.local_get(out);
                    let drop = self.rt("drop");
                    ins.call(drop);
                    ins.br(2);
                    ins.end();
                }
                ins.local_get(at);
                ins.i32_const(1);
                ins.i32_add();
                ins.local_set(at);
                ins.br(0);
                ins.end();
                ins.end();
            }
            // Consumes the array (`heap::rc`); the value goes as an 8-byte word.
            ArraySet => {
                let agg = match &args[0] {
                    Operand::Var(v) => self.inline_array_elem(cx.f.vars[*v].ty).is_some(),
                    Operand::Const(_) => false,
                };
                // A value held in locals is boxed for the call and the box is released after it;
                // the array keeps its own reference (`array_set` dups it) or copies the slots
                // (`array_set_agg`).
                let boxed = match &args[2] {
                    Operand::Var(v) => cx.aggregate(*v).is_some(),
                    Operand::Const(_) => false,
                };
                self.operand(cx, ins, &args[0]);
                self.operand(cx, ins, &args[1]);
                if boxed {
                    let Operand::Var(v) = &args[2] else {
                        unreachable!()
                    };
                    let v = *v;
                    self.box_value(cx, ins, v);
                    if agg {
                        let f = self.rt("array_set_agg");
                        ins.call(f);
                    } else {
                        ins.i64_extend_i32_u();
                        let f = self.rt("array_set");
                        ins.call(f);
                    }
                    // The call leaves the array on the stack; release the temporary box.
                    let Some(box_ref) = cx.box_ref else {
                        self.internal(format!("no box local in {}", cx.f.name));
                        return;
                    };
                    ins.local_get(box_ref);
                    let drop = self.rt("drop");
                    ins.call(drop);
                    self.assign(cx, ins, dst, Repr::Ref);
                    return;
                }
                if agg {
                    // The value stays a reference in the IR; its box's slots are copied into
                    // the inline element, so the caller keeps its own reference.
                    self.operand(cx, ins, &args[2]);
                    let f = self.rt("array_set_agg");
                    ins.call(f);
                    self.assign(cx, ins, dst, Repr::Ref);
                    return;
                }
                match Self::operand_repr(cx.f, &args[2]) {
                    Repr::I32 | Repr::Ref => {
                        self.operand(cx, ins, &args[2]);
                        ins.i64_extend_i32_u();
                    }
                    Repr::I64 => self.operand(cx, ins, &args[2]),
                    Repr::F64 => {
                        self.operand(cx, ins, &args[2]);
                        ins.i64_reinterpret_f64();
                    }
                    Repr::Void => {
                        ins.i64_const(0);
                    }
                }
                let f = self.rt("array_set");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            ArraySlice => {
                for a in args {
                    self.operand(cx, ins, a);
                }
                let f = self.rt("array_slice");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            IntToString => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("int_to_string");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FloatToString => {
                self.operand(cx, ins, &args[0]);
                let f = self.rt("float_to_string");
                ins.call(f);
                self.assign(cx, ins, dst, Repr::Ref);
            }
            FloatParse => self.option_call(cx, ins, dst, args, "float_parse"),
            FloatFloor | FloatCeil | FloatRound | FloatTruncate => {
                self.operand(cx, ins, &args[0]);
                let name = match op {
                    FloatFloor => "float_floor",
                    FloatCeil => "float_ceil",
                    FloatRound => "float_round",
                    _ => "float_truncate",
                };
                let f = self.rt(name);
                ins.call(f);
                self.assign(cx, ins, dst, Repr::I32);
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
                    stride: 0,
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

    /// Copies the inline array element at `index` of `a` into the locals of the value in `dst`
    /// (E1 step 5, E2 step 3). `slots` is the element stride of the array descriptor, so the
    /// address is `a + HDR + 8 + index * slots * 8`. This is the whole read: no box is allocated
    /// and no reference is duped, because an inline all-scalar element has no references.
    fn load_inline_element(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        a: &Operand,
        index: &Operand,
        dst: VarId,
        slots: usize,
    ) {
        let Some(base) = cx.box_ref else {
            self.internal(format!("no box local in {}", cx.f.name));
            return;
        };
        let leaves = match self.leaves(cx.f.vars[dst].ty) {
            Some(leaves) => leaves.to_vec(),
            None => {
                self.internal(format!("an inline element has no layout in {}", cx.f.name));
                return;
            }
        };
        // The address of the element.
        self.operand(cx, ins, a);
        ins.i32_const((HDR + 8) as i32);
        ins.i32_add();
        self.operand(cx, ins, index);
        ins.i32_const((slots * 8) as i32);
        ins.i32_mul();
        ins.i32_add();
        ins.local_set(base);
        for (k, &r) in leaves.iter().enumerate() {
            ins.local_get(base);
            Self::load_mem(ins, k as u32 * 8, r);
            ins.local_set(cx.leaf(dst, k as u32));
        }
    }

    /// Copies the boxed array element at `index` of `a` into the locals of the value in `dst`
    /// (E1 step 5). The array keeps its own reference to the box.
    fn load_boxed_element(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        a: &Operand,
        index: &Operand,
        dst: VarId,
    ) {
        let Some(box_ref) = cx.box_ref else {
            self.internal(format!("no box local in {}", cx.f.name));
            return;
        };
        self.operand(cx, ins, a);
        ins.i32_const((HDR + 8) as i32);
        ins.i32_add();
        self.operand(cx, ins, index);
        ins.i32_const(3);
        ins.i32_shl();
        ins.i32_add();
        ins.i32_load(mem(0, 4));
        ins.local_set(box_ref);
        self.unbox_from(cx, ins, dst, box_ref);
    }

    /// `Array.GetOr` with a destination held by value: the element when the index is in range,
    /// otherwise `d`. Both are copies (E1 step 5), so no box is allocated: with inline array
    /// elements the leaves come straight from the array (E2 step 3), and a boxed element is
    /// unboxed. The bounds test is the boxed lowering's `index >= 0 && index < len`.
    fn array_get_or_value(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: VarId,
        args: &[Operand],
    ) {
        let Operand::Var(a) = &args[0] else {
            self.internal(format!("`ArrayGetOr` of a constant in {}", cx.f.name));
            return;
        };
        let array_ty = cx.f.vars[*a].ty;
        let inline = self.inline_array_elem(array_ty);
        // `index >= 0 && index < array.len`, unsigned for the negative case.
        self.operand(cx, ins, &args[1]);
        ins.i32_const(0);
        ins.i32_ge_s();
        self.operand(cx, ins, &args[1]);
        self.operand(cx, ins, &args[0]);
        ins.i32_load(mem(HDR, 4));
        ins.i32_lt_s();
        ins.i32_and();
        ins.if_(BlockType::Empty);
        match &inline {
            Some(layout) => {
                let slots = layout.slots();
                self.load_inline_element(cx, ins, &args[0], &args[1], dst, slots);
            }
            None => self.load_boxed_element(cx, ins, &args[0], &args[1], dst),
        }
        ins.else_();
        // `d` has the destination's type, so a value held in locals is copied leaf by leaf.
        self.operand(cx, ins, &args[2]);
        self.assign(cx, ins, Some(dst), Repr::Ref);
        ins.end();
    }

    /// `Array.LoadUnchecked` with a destination held by value: copy the element (E1 step 5).
    /// With inline array elements its leaves are loaded straight from the array and no box is
    /// allocated (E2 step 3); the caller has already checked the bounds.
    fn array_load_value(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: VarId,
        args: &[Operand],
    ) {
        let Operand::Var(a) = &args[0] else {
            self.internal(format!(
                "`ArrayLoadUnchecked` of a constant in {}",
                cx.f.name
            ));
            return;
        };
        let array_ty = cx.f.vars[*a].ty;
        match self.inline_array_elem(array_ty) {
            Some(layout) => {
                let slots = layout.slots();
                self.load_inline_element(cx, ins, &args[0], &args[1], dst, slots);
            }
            None => self.load_boxed_element(cx, ins, &args[0], &args[1], dst),
        }
    }

    /// Pushes element `index` of array `a` (an address in a local or on the stack), loaded as
    /// `repr`; a reference is duped, so the value is owned once. `index` must be in range.
    fn array_element(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        a: &Operand,
        index: &Operand,
        repr: Repr,
    ) {
        self.operand(cx, ins, a);
        ins.i32_const((HDR + 8) as i32);
        ins.i32_add();
        self.operand(cx, ins, index);
        ins.i32_const(3);
        ins.i32_shl();
        ins.i32_add();
        match repr {
            Repr::I32 => {
                ins.i32_load(mem(0, 4));
            }
            Repr::I64 => {
                ins.i64_load(mem(0, 8));
            }
            Repr::F64 => {
                ins.f64_load(mem(0, 8));
            }
            // `dup` consumes the reference and returns nothing, so the loaded reference is kept
            // in a scratch local: dup it, then push it again.
            Repr::Ref => {
                ins.i32_load(mem(0, 4));
                let tmp = cx.scratch(Repr::Ref, 0);
                ins.local_set(tmp);
                ins.local_get(tmp);
                let f = self.rt("dup");
                ins.call(f);
                ins.local_get(tmp);
            }
            Repr::Void => unreachable!("no element of the void representation"),
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
