//! Values: the right-hand side of a `let`.

use super::*;

impl Emitter<'_> {
    pub(super) fn value(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, value: &Value) {
        let f = cx.f;
        match value {
            Value::Const(c) => {
                let c = self.constant(c);
                Self::assign(cx, dst, &c);
            }
            Value::Prim { op, ty, args } => {
                let a: Vec<String> = args.iter().map(|o| self.operand(o)).collect();
                let e = prim(*op, *ty, &a);
                Self::assign(cx, dst, &e);
            }
            Value::Call { callee, args, .. } => self.call(cx, dst, callee, args),
            Value::Builtin { op, args } => self.builtin(cx, dst, *op, args),
            Value::Make { ctor, args } => {
                let core = self.core();
                let def = &core.enums[ctor.enum_id];
                if enum_repr(core, ctor.enum_id) == Repr::I32 {
                    Self::assign(cx, dst, &ctor.variant.to_string());
                    return;
                }
                // A record held by value is a struct, without an allocation.
                if let Some(i) = dst.and_then(|d| self.values.index(f.vars[d].ty)) {
                    self.aggregate(cx, dst, i, args);
                    return;
                }
                let key = DescKey {
                    tag: ctor.variant as u32,
                    enum_id: if def.error {
                        ix(ctor.enum_id) as i32
                    } else {
                        -1
                    },
                    name: def.variants[ctor.variant].name.clone(),
                    show: if def.error {
                        self.error_shows.get(&ctor.enum_id).copied()
                    } else {
                        None
                    },
                    refs: Self::refs_of(f, args),
                    value_size: 0,
                };
                let desc = self.desc(key);
                if args.is_empty() {
                    let obj = self.nullary(desc);
                    Self::assign(cx, dst, &obj);
                } else {
                    self.alloc(cx, dst, desc, None, args);
                }
            }
            Value::Field { base, index, .. } | Value::TupleGet { base, index } => {
                let Some(d) = dst else { return };
                if let Some(i) = self.values.index(f.vars[*base].ty) {
                    // A record or tuple held by value: its member, no pointer.
                    let member = self.values.get(i).member(*index).to_owned();
                    cx.w.assign(&var(d), &format!("{}.{member}", var(*base)));
                    return;
                }
                match self.ct(f.vars[d].ty) {
                    CTy::Scalar(r) => {
                        cx.w.assign(&var(d), &format!("{}->{}", var(*base), slot(*index, r)));
                    }
                    // The field of an enum value, which holds a box of the value.
                    CTy::Agg(_) => cx.w.stmt(&format!(
                        "em_unbox_value(&{}, {}->slots[{index}].p)",
                        var(d),
                        var(*base)
                    )),
                }
            }
            Value::Tag(v) => {
                let e = if self.values.index(f.vars[*v].ty).is_some() {
                    // A record has one constructor.
                    "0".to_owned()
                } else if f.vars[*v].repr == Repr::Ref {
                    format!("(int32_t){}", call("em_tag", [var(*v)]))
                } else {
                    var(*v)
                };
                Self::assign(cx, dst, &e);
            }
            Value::Tuple(elems) => {
                if let Some(i) = dst.and_then(|d| self.values.index(f.vars[d].ty)) {
                    self.aggregate(cx, dst, i, elems);
                    return;
                }
                let desc = self.desc(DescKey {
                    tag: 0,
                    enum_id: -1,
                    name: "tuple".into(),
                    show: None,
                    refs: Self::refs_of(f, elems),
                    value_size: 0,
                });
                self.alloc(cx, dst, desc, None, elems);
            }
            Value::List { elems, tail } => {
                let Some(d) = dst else {
                    self.internal(format!("list without a destination in {}", f.name));
                    return;
                };
                let tail = match tail {
                    Some(t) => self.operand(t),
                    None => "NULL".to_owned(),
                };
                cx.w.assign(&var(d), &tail);
                for e in elems.iter().rev() {
                    let r = Self::operand_repr(f, e) == Repr::Ref;
                    let w = self.word(f, e);
                    let flag = if r { "true" } else { "false" };
                    cx.w.assign(&var(d), &call("em_cons", [w, flag.to_owned(), var(d)]));
                }
            }
            Value::ListIsEmpty(v) => Self::assign(cx, dst, &format!("({} == NULL)", var(*v))),
            Value::ListHead(v) => {
                if let Some(d) = dst {
                    match self.ct(f.vars[d].ty) {
                        CTy::Scalar(r) => {
                            cx.w.assign(&var(d), &format!("{}->{}", var(*v), slot(0, r)));
                        }
                        CTy::Agg(_) => {
                            cx.w.stmt(&format!(
                                "em_unbox_value(&{}, {}->slots[0].p)",
                                var(d),
                                var(*v)
                            ));
                        }
                    }
                }
            }
            Value::ListTail(v) => Self::assign(cx, dst, &format!("{}->slots[1].p", var(*v))),
            Value::FnRef(_) | Value::Closure(_) => {
                self.internal(format!("unconverted closure in {}", f.name));
            }
            Value::MakeClosure { function, captures } => {
                let fname = self.fn_names[ix(*function)].clone();
                if captures.is_empty() {
                    let obj = self.static_closure(*function);
                    Self::assign(cx, dst, &obj);
                    return;
                }
                let args: Vec<Operand> = captures.iter().map(|&c| Operand::Var(c)).collect();
                let mut refs = vec![false];
                refs.extend(Self::refs_of(f, &args));
                let desc = self.desc(DescKey {
                    tag: 0,
                    enum_id: -1,
                    name: display_name(&self.m.functions[*function].name).to_owned(),
                    show: None,
                    refs,
                    value_size: 0,
                });
                self.alloc(
                    cx,
                    dst,
                    desc,
                    Some(format!("(em_word){{.fn = (em_fnptr){fname}}}")),
                    &args,
                );
            }
            Value::EnvGet { env, index } => {
                if let Some(d) = dst {
                    match self.ct(f.vars[d].ty) {
                        CTy::Scalar(r) => {
                            cx.w.assign(&var(d), &format!("{}->{}", var(*env), slot(index + 1, r)));
                        }
                        CTy::Agg(_) => cx.w.stmt(&format!(
                            "em_unbox_value(&{}, {}->slots[{}].p)",
                            var(d),
                            var(*env),
                            index + 1
                        )),
                    }
                }
            }
            Value::EvLookup { ev, effect } => {
                let e = call("em_ev_lookup", [var(*ev), ix(*effect).to_string()]);
                Self::assign(cx, dst, &e);
            }
            Value::EvExtend { base, bindings } => {
                let Some(d) = dst else {
                    self.internal(format!("evidence without a destination in {}", f.name));
                    return;
                };
                // The empty table is `NULL`, which `em_ev_new`, `em_ev_lookup`, `em_dup` and
                // `em_drop` accept, so passing it (to a function value whose R needs no table)
                // allocates nothing.
                if base.is_none() && bindings.is_empty() {
                    cx.w.assign(&var(d), "NULL");
                    return;
                }
                let base = base.map_or_else(|| "NULL".to_owned(), var);
                let n = bindings.len();
                cx.w.assign(&var(d), &call("em_ev_new", [base, n.to_string()]));
                for (i, (effect, h)) in bindings.iter().enumerate() {
                    cx.w.stmt(&call(
                        "em_ev_set",
                        [
                            var(d),
                            n.to_string(),
                            i.to_string(),
                            ix(*effect).to_string(),
                            var(*h),
                        ],
                    ));
                }
            }
            Value::Handler { name, ops, .. } => {
                let mut refs = vec![false];
                refs.extend(std::iter::repeat_n(true, ops.len()));
                let desc = self.desc(DescKey {
                    tag: 0,
                    enum_id: -1,
                    name: name.clone(),
                    show: None,
                    refs,
                    value_size: 0,
                });
                let args: Vec<Operand> = ops.iter().map(|&o| Operand::Var(o)).collect();
                self.alloc(cx, dst, desc, Some("(em_word){.i = 0}".into()), &args);
            }
            Value::Fiber { op, args, .. } => self.fiber(cx, dst, *op, args),
            Value::IsError { error, enum_id } => Self::assign(
                cx,
                dst,
                &call("em_is_error", [var(*error), ix(*enum_id).to_string()]),
            ),
            Value::AsError { error, .. } => Self::assign(cx, dst, &var(*error)),
            Value::Runtime { op, args } => {
                let a: Vec<String> = args.iter().map(|o| self.operand(o)).collect();
                let e = match op {
                    RuntimeOp::ShowString => call("em_show_string", &a[..1]),
                    // The parts are a compound literal, so that one String is allocated for
                    // all of them (see `em_string_concat`); it lives until the end of the
                    // enclosing block, which is longer than the call.
                    RuntimeOp::Concat => call(
                        "em_string_concat",
                        [
                            format!("(const em_ref[]){{{}}}", a.join(", ")),
                            a.len().to_string(),
                        ],
                    ),
                    RuntimeOp::DebugPrint => call("em_debug_print", &a[..1]),
                    RuntimeOp::Retire => call("em_retire", &a[..2]),
                    RuntimeOp::Defect => call("em_raise", &a[..1]),
                    RuntimeOp::DefectAt => call("em_raise_at", &a[..2]),
                    RuntimeOp::DefectTake => call("em_defect_take", [""; 0]),
                    RuntimeOp::DefectResume => call("em_defect_resume", &a[..1]),
                    RuntimeOp::Hold => call("em_hold", [""; 0]),
                    RuntimeOp::Unhold => call("em_unhold", [""; 0]),
                    RuntimeOp::Yield => call("em_yield", ["false"]),
                    RuntimeOp::YieldPerform => call("em_yield", ["true"]),
                };
                Self::assign(cx, dst, &e);
            }
            Value::Coerce(v) => {
                let Some(d) = dst else { return };
                let (from, to) = (self.ct(f.vars[*v].ty), self.ct(f.vars[d].ty));
                if from == to {
                    cx.w.assign(&var(d), &var(*v));
                    return;
                }
                match (from, to) {
                    // A value held by value, boxed for `Any`: the box of the value inside the
                    // box of `Any`.
                    (CTy::Agg(i), CTy::Scalar(Repr::Ref)) => {
                        let boxed = self.box_of(&var(*v), i);
                        let e = call("em_box", [word_of(Repr::Ref, &boxed)]);
                        cx.w.assign(&var(d), &e);
                    }
                    (CTy::Scalar(Repr::Ref), CTy::Agg(_)) => {
                        cx.w.stmt(&format!(
                            "em_unbox_value(&{}, {}->slots[0].p)",
                            var(d),
                            var(*v)
                        ));
                    }
                    (CTy::Scalar(a), CTy::Scalar(b)) => {
                        let e = if b == Repr::Ref {
                            call("em_box", [word_of(a, &var(*v))])
                        } else {
                            format!("{}->{}", var(*v), slot(0, b))
                        };
                        cx.w.assign(&var(d), &e);
                    }
                    _ => self.internal(format!("a conversion from a value in {}", f.name)),
                }
            }
        }
    }
}
