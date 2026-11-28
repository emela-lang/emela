//! Builtin operations, emitted inline or as calls into the runtime.

use super::*;

impl Emitter<'_> {
    pub(super) fn builtin(
        &mut self,
        cx: &mut Cx<'_>,
        dst: Option<VarId>,
        op: Builtin,
        args: &[Operand],
    ) {
        let f = cx.f;
        let a: Vec<String> = args.iter().map(|o| self.operand(o)).collect();
        let named = |name: &str| call(name, &a);
        let e = match op {
            Builtin::StringByteSize => named("em_string_byte_size"),
            Builtin::StringAppend => named("em_string_append"),
            Builtin::StringContains => named("em_string_contains"),
            Builtin::StringStartsWith => named("em_string_starts_with"),
            Builtin::StringEndsWith => named("em_string_ends_with"),
            Builtin::StringSplit => named("em_string_split"),
            Builtin::StringJoin => named("em_string_join"),
            Builtin::StringTrim => named("em_string_trim"),
            Builtin::StringCodePoints => named("em_string_code_points"),
            Builtin::StringFromCodePoint => named("em_string_from_code_point"),
            Builtin::StringAsciiLower => named("em_string_ascii_lower"),
            Builtin::StringAsciiUpper => named("em_string_ascii_upper"),
            // A Bytes has the representation of a String (a byte length and the bytes), so the
            // size and the steps between text and bytes are those of a String.
            Builtin::BytesSize => named("em_string_byte_size"),
            Builtin::BytesFromList => named("em_bytes_from_list"),
            Builtin::BytesToList => named("em_bytes_to_list"),
            Builtin::BytesGet => named("em_bytes_get"),
            Builtin::BytesSlice => named("em_bytes_slice"),
            Builtin::BytesConcat => named("em_bytes_concat"),
            Builtin::BytesEncodeUtf8 | Builtin::BytesDecodeValidUtf8 => named("em_bytes_same"),
            // Arrays (`[api.std.array]`): the runtime reads from an array's descriptor whether its
            // elements are references, and an array of a value holds the structs inline, so the
            // descriptor also says the size of one.
            Builtin::ArrayFromList | Builtin::ArrayConcat => {
                let desc = self.array_of(dst.map(|d| f.vars[d].ty));
                let name = if op == Builtin::ArrayFromList {
                    "em_array_from_list"
                } else {
                    "em_array_concat"
                };
                call(name, [a[0].as_str(), desc.as_str()])
            }
            Builtin::ArrayToList => match self.elem_of(f, &args[0]) {
                Some(i) => {
                    let elem = self.elem_desc(i);
                    call("em_array_to_list_agg", [a[0].clone(), elem])
                }
                None => named("em_array_to_list"),
            },
            Builtin::ArraySize => named("em_array_size"),
            Builtin::ArrayGet => match self.elem_of(f, &args[0]) {
                Some(i) => {
                    let elem = self.elem_desc(i);
                    call("em_array_get_agg", [a[0].clone(), a[1].clone(), elem])
                }
                None => named("em_array_get"),
            },
            // The Core IR fusions of `Option.or(Array.get(..), d)` and `match Array.get(..)`.
            // They return a word, unpacked into the destination's representation; a value is
            // written to the destination directly.
            Builtin::ArrayGetOr => match self.elem_of(f, &args[0]) {
                Some(_) => {
                    let Some(dst) = dst else {
                        self.internal(format!("get_or without a destination in {}", f.name));
                        return;
                    };
                    let d = self.operand(&args[2]);
                    cx.w.stmt(&call(
                        "em_array_get_or_agg",
                        [
                            a[0].clone(),
                            a[1].clone(),
                            format!("&{d}"),
                            format!("&{}", var(dst)),
                        ],
                    ));
                    return;
                }
                None => {
                    // A word array: `ref` is the static answer the inline read would otherwise
                    // read from the descriptor (see `elem_refs`).
                    let refs = self.elem_refs(f, args, &a[0]);
                    let d = self.word(f, &args[2]);
                    let value = call("em_array_get_or", [a[0].clone(), a[1].clone(), refs, d]);
                    match dst {
                        Some(d) => format!("{value}.{}", acc(f.vars[d].repr)),
                        None => value,
                    }
                }
            },
            Builtin::ArrayLoadUnchecked => match self.elem_of(f, &args[0]) {
                Some(_) => {
                    let Some(dst) = dst else {
                        self.internal(format!("a load without a destination in {}", f.name));
                        return;
                    };
                    cx.w.stmt(&call(
                        "em_array_load_unchecked_agg",
                        [a[0].clone(), a[1].clone(), format!("&{}", var(dst))],
                    ));
                    return;
                }
                None => {
                    let refs = self.elem_refs(f, args, &a[0]);
                    let value = call(
                        "em_array_load_unchecked",
                        [a[0].clone(), a[1].clone(), refs],
                    );
                    match dst {
                        Some(d) => format!("{value}.{}", acc(f.vars[d].repr)),
                        None => value,
                    }
                }
            },
            // Consumes the array (`heap::rc`); the value is passed as a word, or as the struct.
            Builtin::ArraySet => match self.elem_of(f, &args[0]) {
                Some(_) => {
                    let value = self.operand(&args[2]);
                    call(
                        "em_array_set_agg",
                        [a[0].clone(), a[1].clone(), format!("&{value}")],
                    )
                }
                None => {
                    let value = self.word(f, &args[2]);
                    call("em_array_set", [a[0].clone(), a[1].clone(), value])
                }
            },
            // The Core IR fusion of `Array.from_list(List.map(List.range(lo, hi), f))`: one array,
            // filled by calling `f` for each position. `f` is pure and takes one `Int`, so the
            // closure call is the environment and that value, returning the element's
            // representation. `em_array_new` leaves the elements uninitialized, so a defect from
            // `f` stops the loop at the first one, shortens the array to the elements already
            // written and releases it: nothing else releases the destination of a statement that
            // defects, because a destination is not live before its own statement. Nothing else
            // leaves the loop early (`f` neither fails nor suspends, and the builtin has no
            // interruption point).
            Builtin::ArrayMapRange => {
                let Some(d) = dst else {
                    self.internal(format!(
                        "`ArrayMapRange` without a destination in {}",
                        f.name
                    ));
                    return;
                };
                let Type::Array(elem_ty) = *self.core().ty(f.vars[d].ty) else {
                    self.internal(format!(
                        "`ArrayMapRange` without an array destination in {}",
                        f.name
                    ));
                    return;
                };
                // The element is the type argument of the array: a record or tuple held by value
                // gives a struct to the closure and a stride to the array; a type parameter that
                // became `()` after monomorphization has no representation, so the loop calls `f`
                // for each position and stores nothing (the slots stay zero).
                let elem = self.ct(elem_ty);
                let elem_c = self.cty(elem).to_owned();
                let desc = self.array_of(Some(f.vars[d].ty));
                let array = var(d);
                let lo = format!("{array}_lo");
                let hi = format!("{array}_hi");
                let fun = format!("{array}_f");
                let count = format!("{array}_n");
                let i = format!("{array}_i");
                // The count is 64-bit, as in the runtime's array functions: `hi - lo` is exact
                // there even for a range longer than `Int.MAX` (which cannot be an array anyway,
                // and is out of memory). The argument stays an `Int` by wrapping, as `i + 1`
                // does.
                let step = format!(
                    "(({elem_c} (*)(em_ref, int32_t))em_closure_fn({fun}))({fun}, \
                     (int32_t)((uint32_t){lo} + (uint32_t){i}))"
                );
                cx.w.block(|w| {
                    w.decl("int32_t", &lo, Some(&a[0]));
                    w.decl("int32_t", &hi, Some(&a[1]));
                    w.decl("em_ref", &fun, Some(&a[2]));
                    w.decl(
                        "int64_t",
                        &count,
                        Some(&format!("(int64_t){hi} - (int64_t){lo}")),
                    );
                    w.decl("int64_t", &i, Some("0"));
                    w.if_(&format!("{count} < 0"), |w| w.assign(&count, "0"));
                    w.assign(&array, &call("em_array_new", [count.clone(), desc]));
                    w.for_(
                        &format!("{i} = 0"),
                        &format!("{i} < {count}"),
                        &format!("{i}++"),
                        |w| {
                            match elem {
                                CTy::Scalar(Repr::Void) => w.stmt(&step),
                                // One element per slot, of an array that the descriptor sized.
                                CTy::Scalar(r) => {
                                    w.assign(&format!("{array}->slots[1 + {i}].{}", acc(r)), &step)
                                }
                                // The bytes of the value, one after another from slot 1.
                                CTy::Agg(_) => w.assign(
                                    &format!("(({elem_c} *)&{array}->slots[1])[{i}]"),
                                    &step,
                                ),
                            };
                            // A defect from `f` (nobody wrote it into the type) ends the loop
                            // where it happens: the elements from `i` on were never written, so
                            // the length becomes the count written. The array is the statement's
                            // destination, which a landing does not own on the defect path (a
                            // destination is not live before its own statement), so the array is
                            // released here, and the landing unwinds without it.
                            w.if_("em_defect != NULL", |w| {
                                w.assign(&format!("{array}->slots[0].l"), &i);
                                w.stmt(&call("em_drop", [array.clone()]));
                                w.stmt("break");
                            });
                        },
                    );
                });
                return;
            }
            Builtin::ArraySlice => named("em_array_slice"),
            Builtin::IntToString => named("em_int_to_string"),
            Builtin::IntParse => named("em_int_parse"),
            Builtin::IntCheckedAdd => named("em_int_checked_add"),
            Builtin::IntBitAnd | Builtin::Int64BitAnd => format!("({} & {})", a[0], a[1]),
            Builtin::IntBitOr | Builtin::Int64BitOr => format!("({} | {})", a[0], a[1]),
            Builtin::IntBitXor | Builtin::Int64BitXor => format!("({} ^ {})", a[0], a[1]),
            Builtin::IntBitNot | Builtin::Int64BitNot => format!("(~{})", a[0]),
            Builtin::IntShiftLeft => named("em_ishl"),
            Builtin::IntShiftRight => named("em_ishr"),
            Builtin::IntShiftRightUnsigned => named("em_iushr"),
            Builtin::Int64FromInt => format!("(int64_t){}", a[0]),
            Builtin::Int64ToString => named("em_int64_to_string"),
            Builtin::Int64ToInt => format!("(int32_t)(uint32_t)(uint64_t){}", a[0]),
            Builtin::Int64ShiftLeft => named("em_lshl"),
            Builtin::Int64ShiftRight => named("em_lshr"),
            Builtin::Int64ShiftRightUnsigned => named("em_lushr"),
            Builtin::FloatToString => named("em_float_to_string"),
            Builtin::FloatFromInt => format!("(double){}", a[0]),
            Builtin::FloatParse => named("em_float_parse"),
            Builtin::FloatFloor => named("em_float_floor"),
            Builtin::FloatCeil => named("em_float_ceil"),
            Builtin::FloatRound => named("em_float_round"),
            Builtin::FloatTruncate => named("em_float_truncate"),
            Builtin::CellNew => {
                let Some(d) = dst else { return };
                let content = args
                    .first()
                    .map_or(Repr::Void, |o| Self::operand_repr(f, o));
                let w = match args.first() {
                    Some(o) => self.word(f, o),
                    None => "(em_word){.l = 0}".to_owned(),
                };
                let desc = if content == Repr::Ref {
                    "&em_desc_cell_ref"
                } else {
                    "&em_desc_cell_val"
                };
                cx.w.assign(&var(d), &call("em_alloc", [desc, "1"]));
                cx.w.assign(&format!("{}->slots[0]", var(d)), &w);
                if content == Repr::Ref {
                    cx.w.stmt(&call("em_dup", &a[..1]));
                }
                return;
            }
            Builtin::CellGet => {
                if let Some(d) = dst {
                    let r = f.vars[d].repr;
                    cx.w.assign(&var(d), &format!("{}->{}", a[0], slot(0, r)));
                    if r == Repr::Ref {
                        cx.w.stmt(&call("em_dup", [var(d)]));
                    }
                }
                return;
            }
            Builtin::CellSet => {
                let Some(v) = args.get(1) else { return };
                match Self::operand_repr(f, v) {
                    Repr::Ref => {
                        cx.w.stmt(&call("em_dup", &a[1..2]));
                        cx.w.block(|w| {
                            w.decl("em_ref", "old", Some(&format!("{}->slots[0].p", a[0])));
                            w.assign(&format!("{}->slots[0].p", a[0]), &a[1]);
                            w.stmt("em_drop(old)");
                        });
                    }
                    Repr::Void => {}
                    r => cx.w.assign(&format!("{}->{}", a[0], slot(0, r)), &a[1]),
                }
                return;
            }
            Builtin::StringLength
            | Builtin::StringChars
            | Builtin::Panic
            | Builtin::Todo
            | Builtin::AssertFail
            | Builtin::Dbg
            | Builtin::Inspect
            | Builtin::ShowNested
            | Builtin::ShowString
            | Builtin::DebugPrint => {
                self.internal(format!("builtin {op:?} left in {}", f.name));
                return;
            }
        };
        Self::assign(cx, dst, &e);
    }
}
