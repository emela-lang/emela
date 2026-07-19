//! ARC retain/release insertion (spec 0048).
//!
//! [`insert_rc_ops`] rewrites a lowered [`IrProgram`] so every heap value is
//! reference-counted: [`IrExpr::Retain`] in owned positions, [`IrExpr::Release`]
//! at scope exits. The wasm backend runs it after lowering and the tail-call
//! rewrite (spec 0045); backends that reclaim by other means never invoke it
//! and treat the nodes as transparent (spec 0048 A9).
//!
//! ## Ownership convention (owned-args)
//!
//! Every heap-typed expression evaluates to an *owned* (+1) reference. The one
//! exception is a bare `Var`, which is a *borrow* of a binding that strictly
//! outlives it; the pass wraps it in `retain` only in owned positions: call and
//! tail-call arguments, constructor fields (enum payload, record fields, array
//! elements, `array_push`'s element), the thrown value, a `let`'s right-hand
//! side, and every tail (returned) value. A function owns its heap parameters
//! and releases them on every exit. Borrowed positions — intrinsic/platform
//! arguments, scrutinees, guards, conditions, operands — read the value in
//! place; a non-`Var` heap operand there is bound to a fresh `$rc` temporary
//! released right after its consumer.
//!
//! Owned-args is forced by spec 0045: once a self tail call becomes a jump, no
//! caller-side release site exists, so borrowed arguments would accumulate
//! obligations across iterations (violating 0048 A5). With owned arguments the
//! jump transfers ownership into the next iteration's parameters after the
//! current frame's bindings are released.
//!
//! ## Soundness sketch
//!
//! Bindings are alpha-renamed first, so `release x` always names one binding.
//! A managed binding (heap-typed `let`, parameter, or `$rc` temporary) is
//! released exactly once per control path, at the tails of its scope; a jump
//! out of the scope (`TailSelfCall`, a `throw` on a tail path) carries the
//! releases immediately above it, after its operands were evaluated into
//! temporaries. Match/catch payload bindings are pure borrows of a scrutinee
//! that outlives the arm. Acyclicity (spec 0024) makes count-zero equivalent
//! to unreachability.
//!
//! What this pass does *not* cover (backend duty, spec 0048 A7): releasing
//! live bindings when a throwing call or `?` unwinds through the backend's
//! error channel, and the caught error value's own count.

use crate::ir::{IrArm, IrExpr, IrParam, IrPattern, IrProgram, QuestionMode};
use crate::types::Type;

/// Whether values of `ty` are heap pointers managed by RC (spec 0048).
pub fn is_heap(ty: &Type) -> bool {
    matches!(
        ty,
        Type::String
            | Type::Array(_)
            | Type::Record
            | Type::Enum(_, _)
            | Type::Option(_)
            | Type::Function(_)
            | Type::OpaqueFunction
    )
}

/// Inserts retain/release across the program (spec 0048 A8).
pub fn insert_rc_ops(program: &mut IrProgram) {
    let mut renamer = Renamer::default();
    for function in &mut program.functions {
        renamer.rename_fn(&mut function.params, &mut function.body);
    }
    let mut cx = Cx::default();
    for function in &mut program.functions {
        let body = std::mem::replace(&mut function.body, IrExpr::Unit);
        function.body = cx.fn_body(&function.params, body);
    }
}

// ---------------------------------------------------------------------------
// Alpha-renaming
//
// `Release` targets a binding by name, so shadowing (`let x = f(x)`, a match
// arm binding over an outer `let`) must be resolved first: every binder that
// shadows a visible name gets a fresh `name#N`. `#` cannot appear in an Emela
// identifier, so renamed binders never collide with user names.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Renamer {
    suffixes: std::collections::HashMap<String, usize>,
}

type Env = Vec<(String, String)>;

impl Renamer {
    fn rename_fn(&mut self, params: &mut [IrParam], body: &mut IrExpr) {
        let mut env: Env = Vec::new();
        for param in params.iter_mut() {
            param.name = self.bind(&param.name, &mut env);
        }
        self.expr(body, &mut env);
    }

    fn bind(&mut self, name: &str, env: &mut Env) -> String {
        let unique = if env.iter().any(|(n, _)| n == name) {
            let suffix = self.suffixes.entry(name.to_string()).or_insert(0);
            *suffix += 1;
            format!("{name}#{suffix}")
        } else {
            name.to_string()
        };
        env.push((name.to_string(), unique.clone()));
        unique
    }

    fn arm(&mut self, arm: &mut IrArm, env: &mut Env) {
        let mark = env.len();
        match &mut arm.pattern {
            IrPattern::Variant { bindings, .. } => {
                for binding in bindings.iter_mut().flatten() {
                    binding.0 = self.bind(&binding.0.clone(), env);
                }
            }
            IrPattern::Wildcard { binding } => {
                if let Some((name, _)) = binding {
                    *name = self.bind(&name.clone(), env);
                }
            }
        }
        if let Some(guard) = &mut arm.guard {
            self.expr(guard, env);
        }
        self.expr(&mut arm.body, env);
        env.truncate(mark);
    }

    fn expr(&mut self, expr: &mut IrExpr, env: &mut Env) {
        match expr {
            IrExpr::Var { name, .. } => {
                if let Some((_, unique)) = env.iter().rev().find(|(n, _)| n == name) {
                    *name = unique.clone();
                }
            }
            IrExpr::Let {
                name, value, next, ..
            } => {
                let mark = env.len();
                self.expr(value, env);
                *name = self.bind(&name.clone(), env);
                self.expr(next, env);
                env.truncate(mark);
            }
            IrExpr::Fn {
                params,
                captures,
                body,
                ..
            } => {
                // A capture names the *enclosing* binding (backends read it in
                // the enclosing scope and re-bind it under the same name in
                // the lambda), so it follows the enclosing renaming; the body
                // then resolves the original name to that same unique one.
                for capture in captures.iter_mut() {
                    if let Some((_, unique)) = env.iter().rev().find(|(n, _)| n == &capture.name) {
                        capture.name = unique.clone();
                    }
                }
                let mark = env.len();
                for param in params.iter_mut() {
                    param.name = self.bind(&param.name.clone(), env);
                }
                self.expr(body, env);
                env.truncate(mark);
            }
            IrExpr::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee, env);
                for arm in arms {
                    self.arm(arm, env);
                }
            }
            IrExpr::Try { body, arms, .. } => {
                self.expr(body, env);
                for arm in arms {
                    self.arm(arm, env);
                }
            }
            IrExpr::Array { elems, .. } => elems.iter_mut().for_each(|e| self.expr(e, env)),
            IrExpr::Call { callee, args, .. } => {
                self.expr(callee, env);
                args.iter_mut().for_each(|a| self.expr(a, env));
            }
            IrExpr::Platform { args, .. }
            | IrExpr::Intrinsic { args, .. }
            | IrExpr::TailSelfCall { args, .. } => {
                args.iter_mut().for_each(|a| self.expr(a, env));
            }
            IrExpr::If {
                cond, then, els, ..
            } => {
                self.expr(cond, env);
                self.expr(then, env);
                self.expr(els, env);
            }
            IrExpr::Binary { left, right, .. } | IrExpr::Concat { left, right } => {
                self.expr(left, env);
                self.expr(right, env);
            }
            IrExpr::EnumValue { payload, .. } => {
                payload.iter_mut().for_each(|e| self.expr(e, env));
            }
            IrExpr::RecordValue { fields, .. } => {
                fields.iter_mut().for_each(|e| self.expr(e, env));
            }
            IrExpr::FieldAccess { target, .. } => self.expr(target, env),
            IrExpr::Throw { value } | IrExpr::Question { value, .. } => self.expr(value, env),
            IrExpr::Panic { message } => self.expr(message, env),
            IrExpr::Retain { value } => self.expr(value, env),
            IrExpr::Release { next, .. } => self.expr(next, env),
            IrExpr::Int(_)
            | IrExpr::Float(_)
            | IrExpr::Bool(_)
            | IrExpr::String(_)
            | IrExpr::Char(_)
            | IrExpr::Unit
            | IrExpr::FunctionRef { .. } => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------

/// How an operand position treats a heap value.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// The consumer takes ownership (+1): a bare `Var` is retained.
    Owned,
    /// The consumer only reads: a `Var` stays bare; any other heap operand is
    /// bound to a `$rc` temporary and released after the consumer.
    Borrowed,
}

/// An operand hoisted into a `let` above its consumer. `release` marks a
/// borrowed temporary (released after the consumer); an owned one transfers.
struct Hoisted {
    name: String,
    ty: Type,
    value: IrExpr,
    release: bool,
}

#[derive(Default)]
struct Cx {
    counter: usize,
}

/// A literal or variable read: safe to evaluate later than written order.
/// (`retain` on a read commutes with everything except a release of the same
/// object, and releases always come after the consuming node.)
fn reorder_safe(expr: &IrExpr) -> bool {
    matches!(
        expr,
        IrExpr::Int(_)
            | IrExpr::Float(_)
            | IrExpr::Bool(_)
            | IrExpr::Char(_)
            | IrExpr::Unit
            | IrExpr::String(_)
            | IrExpr::Var { .. }
    )
}

fn literal(expr: &IrExpr) -> bool {
    matches!(
        expr,
        IrExpr::Int(_)
            | IrExpr::Float(_)
            | IrExpr::Bool(_)
            | IrExpr::Char(_)
            | IrExpr::Unit
            | IrExpr::String(_)
    )
}

impl Cx {
    fn fresh(&mut self) -> String {
        let n = self.counter;
        self.counter += 1;
        format!("$rc{n}")
    }

    /// A function (or lambda) body: the owned tail value, with every
    /// heap-typed parameter released on every path out.
    fn fn_body(&mut self, params: &[IrParam], body: IrExpr) -> IrExpr {
        let mut out = self.owned(body);
        for param in params {
            if is_heap(&param.ty) {
                out = self.release_at_tails(out, &param.name, &param.ty);
            }
        }
        out
    }

    /// Rewrites `e` so it evaluates to an owned (+1) reference (or a plain
    /// value for non-heap types), with every temporary it creates internally
    /// released within it.
    fn owned(&mut self, e: IrExpr) -> IrExpr {
        match e {
            IrExpr::Int(_)
            | IrExpr::Float(_)
            | IrExpr::Bool(_)
            | IrExpr::Char(_)
            | IrExpr::Unit
            | IrExpr::String(_)
            | IrExpr::FunctionRef { .. } => e,

            IrExpr::Var { name, ty } => {
                if is_heap(&ty) {
                    IrExpr::Retain {
                        value: Box::new(IrExpr::Var { name, ty }),
                    }
                } else {
                    IrExpr::Var { name, ty }
                }
            }

            IrExpr::Let {
                name,
                value_ty,
                value,
                next,
            } => {
                let value = self.owned(*value);
                let next = self.owned(*next);
                let next = if is_heap(&value_ty) {
                    self.release_at_tails(next, &name, &value_ty)
                } else {
                    next
                };
                IrExpr::Let {
                    name,
                    value_ty,
                    value: Box::new(value),
                    next: Box::new(next),
                }
            }

            IrExpr::Call { callee, args, ret } => {
                let (hoisted, args) = self.operands(args, |_| Mode::Owned);
                debug_assert!(hoisted.is_empty(), "owned operands are never hoisted");
                match *callee {
                    // A direct target or a borrowed closure binding.
                    callee @ (IrExpr::FunctionRef { .. } | IrExpr::Var { .. }) => IrExpr::Call {
                        callee: Box::new(callee),
                        args,
                        ret,
                    },
                    // Any other callee expression: own it for the call, then
                    // release it.
                    other => {
                        let ty = other.ty();
                        let value = self.owned(other);
                        let name = self.fresh();
                        let node = IrExpr::Call {
                            callee: Box::new(IrExpr::Var {
                                name: name.clone(),
                                ty: ty.clone(),
                            }),
                            args,
                            ret,
                        };
                        self.wrap(
                            vec![Hoisted {
                                name,
                                ty,
                                value,
                                release: true,
                            }],
                            node,
                        )
                    }
                }
            }

            IrExpr::TailSelfCall { args, ty } => {
                // Releases are inserted directly above the jump (after this
                // node is built), so every argument that reads the heap must
                // already be evaluated into a temporary by then. Only literals
                // and non-heap variable reads stay inline.
                let mut lets: Vec<(String, Type, IrExpr)> = Vec::new();
                let args = args
                    .into_iter()
                    .map(|arg| {
                        let arg_ty = arg.ty();
                        let inline = literal(&arg)
                            || (matches!(arg, IrExpr::Var { .. }) && !is_heap(&arg_ty));
                        if inline {
                            arg
                        } else {
                            let value = self.owned(arg);
                            let name = self.fresh();
                            lets.push((name.clone(), arg_ty.clone(), value));
                            IrExpr::Var { name, ty: arg_ty }
                        }
                    })
                    .collect();
                let mut out = IrExpr::TailSelfCall { args, ty };
                for (name, ty, value) in lets.into_iter().rev() {
                    out = IrExpr::Let {
                        name,
                        value_ty: ty,
                        value: Box::new(value),
                        next: Box::new(out),
                    };
                }
                out
            }

            IrExpr::Intrinsic { name, args, ret } => {
                // Intrinsic arguments are borrows (the backend reads bytes or
                // copies), with one exception: `array_push` stores its element
                // into the fresh array, consuming it (spec 0007/0021).
                let owned_elem = name == "array_push";
                let (hoisted, args) = self.operands(args, |i| {
                    if owned_elem && i == 1 {
                        Mode::Owned
                    } else {
                        Mode::Borrowed
                    }
                });
                let node = IrExpr::Intrinsic { name, args, ret };
                self.wrap(hoisted, node)
            }

            IrExpr::Platform {
                name,
                args,
                ret,
                throws,
            } => {
                // The host reads (or copies) its arguments; results are fresh
                // guest allocations, owned like any constructor.
                let (hoisted, args) = self.operands(args, |_| Mode::Borrowed);
                let node = IrExpr::Platform {
                    name,
                    args,
                    ret,
                    throws,
                };
                self.wrap(hoisted, node)
            }

            IrExpr::Binary {
                op,
                ty,
                left,
                right,
            } => {
                let (hoisted, mut ops) = self.operands(vec![*left, *right], |_| Mode::Borrowed);
                let right = ops.pop().expect("two operands");
                let left = ops.pop().expect("two operands");
                let node = IrExpr::Binary {
                    op,
                    ty,
                    left: Box::new(left),
                    right: Box::new(right),
                };
                self.wrap(hoisted, node)
            }

            IrExpr::Concat { left, right } => {
                let (hoisted, mut ops) = self.operands(vec![*left, *right], |_| Mode::Borrowed);
                let right = ops.pop().expect("two operands");
                let left = ops.pop().expect("two operands");
                let node = IrExpr::Concat {
                    left: Box::new(left),
                    right: Box::new(right),
                };
                self.wrap(hoisted, node)
            }

            IrExpr::Array { elem_ty, elems } => {
                let (hoisted, elems) = self.operands(elems, |_| Mode::Owned);
                debug_assert!(hoisted.is_empty(), "owned operands are never hoisted");
                IrExpr::Array { elem_ty, elems }
            }

            IrExpr::EnumValue {
                ty,
                variant,
                tag,
                payload,
            } => {
                let (hoisted, payload) = self.operands(payload, |_| Mode::Owned);
                debug_assert!(hoisted.is_empty(), "owned operands are never hoisted");
                IrExpr::EnumValue {
                    ty,
                    variant,
                    tag,
                    payload,
                }
            }

            IrExpr::RecordValue { ty, fields } => {
                let (hoisted, fields) = self.operands(fields, |_| Mode::Owned);
                debug_assert!(hoisted.is_empty(), "owned operands are never hoisted");
                IrExpr::RecordValue { ty, fields }
            }

            IrExpr::FieldAccess {
                target,
                index,
                field_ty,
            } => {
                // The record is borrowed for the load; the backend retains the
                // loaded field, making the result owned.
                let (hoisted, mut targets) = self.operands(vec![*target], |_| Mode::Borrowed);
                let target = targets.pop().expect("one operand");
                let node = IrExpr::FieldAccess {
                    target: Box::new(target),
                    index,
                    field_ty,
                };
                self.wrap(hoisted, node)
            }

            IrExpr::If {
                cond,
                then,
                els,
                ty,
            } => IrExpr::If {
                cond: Box::new(self.owned(*cond)),
                then: Box::new(self.owned(*then)),
                els: Box::new(self.owned(*els)),
                ty,
            },

            IrExpr::Match {
                scrutinee,
                arms,
                ty,
            } => {
                let arms: Vec<IrArm> = arms
                    .into_iter()
                    .map(|arm| IrArm {
                        pattern: arm.pattern,
                        guard: arm.guard.map(|g| self.owned(g)),
                        body: self.owned(arm.body),
                    })
                    .collect();
                match *scrutinee {
                    // A borrowed binding: pattern bindings borrow through it.
                    scrutinee @ IrExpr::Var { .. } => IrExpr::Match {
                        scrutinee: Box::new(scrutinee),
                        arms,
                        ty,
                    },
                    // Own the scrutinee for the match, release it in each arm.
                    other => {
                        let sty = other.ty();
                        let value = self.owned(other);
                        let name = self.fresh();
                        let arms = arms
                            .into_iter()
                            .map(|arm| IrArm {
                                pattern: arm.pattern,
                                guard: arm.guard,
                                body: self.release_at_tails(arm.body, &name, &sty),
                            })
                            .collect();
                        IrExpr::Let {
                            name: name.clone(),
                            value_ty: sty.clone(),
                            value: Box::new(value),
                            next: Box::new(IrExpr::Match {
                                scrutinee: Box::new(IrExpr::Var { name, ty: sty }),
                                arms,
                                ty,
                            }),
                        }
                    }
                }
            }

            IrExpr::Try { body, arms, ty } => IrExpr::Try {
                body: Box::new(self.owned(*body)),
                arms: arms
                    .into_iter()
                    .map(|arm| IrArm {
                        pattern: arm.pattern,
                        guard: arm.guard.map(|g| self.owned(g)),
                        body: self.owned(arm.body),
                    })
                    .collect(),
                ty,
            },

            IrExpr::Question { value, mode, ty } => match mode {
                // The throwing call already yields its unwrapped, owned value.
                QuestionMode::Throws => IrExpr::Question {
                    value: Box::new(self.owned(*value)),
                    mode,
                    ty,
                },
                // The option is inspected (borrowed); the backend retains the
                // payload on the `Some` path, making the result owned.
                QuestionMode::Option => {
                    let (hoisted, mut values) = self.operands(vec![*value], |_| Mode::Borrowed);
                    let value = values.pop().expect("one operand");
                    let node = IrExpr::Question {
                        value: Box::new(value),
                        mode,
                        ty,
                    };
                    self.wrap(hoisted, node)
                }
            },

            IrExpr::Throw { value } => {
                // Atomize the error so releases can slot between its
                // evaluation and the raise (a throw is a jump; see
                // `release_at_tails`).
                let value = self.owned(*value);
                if literal(&value) {
                    IrExpr::Throw {
                        value: Box::new(value),
                    }
                } else {
                    let ty = value.ty();
                    let name = self.fresh();
                    IrExpr::Let {
                        name: name.clone(),
                        value_ty: ty.clone(),
                        value: Box::new(value),
                        next: Box::new(IrExpr::Throw {
                            value: Box::new(IrExpr::Var { name, ty }),
                        }),
                    }
                }
            }

            IrExpr::Panic { message } => {
                // The trap ends the instance; a borrowed read suffices.
                let message = match *message {
                    m @ IrExpr::Var { .. } => m,
                    other => self.owned(other),
                };
                IrExpr::Panic {
                    message: Box::new(message),
                }
            }

            IrExpr::Fn {
                params,
                ret,
                throws,
                effects,
                captures,
                body,
            } => {
                // A lambda is its own function: its heap parameters are owned
                // by its frame. Captures are borrows of the environment (the
                // closure owns them; its drop glue releases them).
                let body = self.fn_body(&params, *body);
                IrExpr::Fn {
                    params,
                    ret,
                    throws,
                    effects,
                    captures,
                    body: Box::new(body),
                }
            }

            IrExpr::Retain { .. } | IrExpr::Release { .. } => {
                unreachable!("insert_rc_ops runs once, before any RC nodes exist")
            }
        }
    }

    /// Rewrites an operand list, preserving left-to-right evaluation (spec
    /// 0003). Borrowed heap non-atoms are hoisted (and released after the
    /// consumer); anything effectful that would otherwise evaluate after a
    /// hoisted operand is hoisted too, without a release.
    fn operands(
        &mut self,
        args: Vec<IrExpr>,
        mode: impl Fn(usize) -> Mode,
    ) -> (Vec<Hoisted>, Vec<IrExpr>) {
        let needs_rc_hoist: Vec<bool> = args
            .iter()
            .enumerate()
            .map(|(i, arg)| {
                mode(i) == Mode::Borrowed
                    && is_heap(&arg.ty())
                    && !matches!(arg, IrExpr::Var { .. } | IrExpr::String(_))
            })
            .collect();
        let last_hoist = needs_rc_hoist.iter().rposition(|&b| b);
        let mut hoisted = Vec::new();
        let out = args
            .into_iter()
            .enumerate()
            .map(|(i, arg)| {
                let ty = arg.ty();
                if needs_rc_hoist[i] {
                    let value = self.owned(arg);
                    let name = self.fresh();
                    hoisted.push(Hoisted {
                        name: name.clone(),
                        ty: ty.clone(),
                        value,
                        release: true,
                    });
                    IrExpr::Var { name, ty }
                } else if last_hoist.is_some_and(|l| i < l) && !reorder_safe(&arg) {
                    // A later operand is hoisted above the consumer; hoist
                    // this one too so it still evaluates first.
                    let value = self.inline_operand(arg, mode(i));
                    let name = self.fresh();
                    hoisted.push(Hoisted {
                        name: name.clone(),
                        ty: ty.clone(),
                        value,
                        release: false,
                    });
                    IrExpr::Var { name, ty }
                } else {
                    self.inline_operand(arg, mode(i))
                }
            })
            .collect();
        (hoisted, out)
    }

    fn inline_operand(&mut self, arg: IrExpr, mode: Mode) -> IrExpr {
        match mode {
            Mode::Owned => self.owned(arg),
            Mode::Borrowed => {
                if is_heap(&arg.ty()) {
                    // Only a `Var` or a static literal reaches here inline.
                    arg
                } else {
                    self.owned(arg)
                }
            }
        }
    }

    /// Wraps `node` with the hoisted operand bindings and, after it, the
    /// releases of the borrowed ones.
    fn wrap(&mut self, hoisted: Vec<Hoisted>, node: IrExpr) -> IrExpr {
        if hoisted.is_empty() {
            return node;
        }
        let releases: Vec<(String, Type)> = hoisted
            .iter()
            .filter(|h| h.release)
            .map(|h| (h.name.clone(), h.ty.clone()))
            .collect();
        let mut out = if releases.is_empty() {
            node
        } else {
            let node_ty = node.ty();
            let result = self.fresh();
            let mut tail = IrExpr::Var {
                name: result.clone(),
                ty: node_ty.clone(),
            };
            for (name, ty) in releases {
                tail = IrExpr::Release {
                    name,
                    ty,
                    next: Box::new(tail),
                };
            }
            IrExpr::Let {
                name: result,
                value_ty: node_ty,
                value: Box::new(node),
                next: Box::new(tail),
            }
        };
        for h in hoisted.into_iter().rev() {
            out = IrExpr::Let {
                name: h.name,
                value_ty: h.ty,
                value: Box::new(h.value),
                next: Box::new(out),
            };
        }
        out
    }

    /// Inserts `release name` at every tail of `e` — the points where `e`
    /// produces its value or jumps out of the scope.
    fn release_at_tails(&mut self, e: IrExpr, name: &str, ty: &Type) -> IrExpr {
        match e {
            IrExpr::Let {
                name: n,
                value_ty,
                value,
                next,
            } => IrExpr::Let {
                name: n,
                value_ty,
                value,
                next: Box::new(self.release_at_tails(*next, name, ty)),
            },
            IrExpr::Release {
                name: n,
                ty: t,
                next,
            } => IrExpr::Release {
                name: n,
                ty: t,
                next: Box::new(self.release_at_tails(*next, name, ty)),
            },
            IrExpr::If {
                cond,
                then,
                els,
                ty: node_ty,
            } => IrExpr::If {
                cond,
                then: Box::new(self.release_at_tails(*then, name, ty)),
                els: Box::new(self.release_at_tails(*els, name, ty)),
                ty: node_ty,
            },
            IrExpr::Match {
                scrutinee,
                arms,
                ty: node_ty,
            } => IrExpr::Match {
                scrutinee,
                arms: arms
                    .into_iter()
                    .map(|arm| IrArm {
                        pattern: arm.pattern,
                        guard: arm.guard,
                        body: self.release_at_tails(arm.body, name, ty),
                    })
                    .collect(),
                ty: node_ty,
            },
            IrExpr::Try {
                body,
                arms,
                ty: node_ty,
            } => {
                // A catch arm is a tail position (spec 0045 T1): a jump out of
                // it must carry the release. The value paths (the body's value
                // or an arm's value) flow out of the whole `try` and hit the
                // single release below — each path releases exactly once.
                let arms = arms
                    .into_iter()
                    .map(|arm| IrArm {
                        pattern: arm.pattern,
                        guard: arm.guard,
                        body: release_at_tail_jumps(arm.body, name, ty),
                    })
                    .collect();
                self.leaf_release(
                    IrExpr::Try {
                        body,
                        arms,
                        ty: node_ty,
                    },
                    name,
                    ty,
                )
            }
            // A jump out of the scope: release right before it (its operands
            // are already in temporaries).
            jump @ (IrExpr::TailSelfCall { .. } | IrExpr::Throw { .. }) => IrExpr::Release {
                name: name.to_string(),
                ty: ty.clone(),
                next: Box::new(jump),
            },
            // A literal or variable read: the release cannot invalidate it (a
            // bare heap `Var` here is always an owned `$rc` transfer temp).
            leaf if reorder_safe(&leaf) => IrExpr::Release {
                name: name.to_string(),
                ty: ty.clone(),
                next: Box::new(leaf),
            },
            leaf => self.leaf_release(leaf, name, ty),
        }
    }

    /// `let $rc = leaf; release name; $rc` — evaluate the tail value first,
    /// then release.
    fn leaf_release(&mut self, leaf: IrExpr, name: &str, ty: &Type) -> IrExpr {
        let leaf_ty = leaf.ty();
        let result = self.fresh();
        IrExpr::Let {
            name: result.clone(),
            value_ty: leaf_ty.clone(),
            value: Box::new(leaf),
            next: Box::new(IrExpr::Release {
                name: name.to_string(),
                ty: ty.clone(),
                next: Box::new(IrExpr::Var {
                    name: result,
                    ty: leaf_ty,
                }),
            }),
        }
    }
}

/// Like [`Cx::release_at_tails`], but only wraps the *jumps* (`TailSelfCall`,
/// `throw`) and leaves value tails untouched. Used inside a `try`'s catch
/// arms, whose value paths are released after the `try` as a whole.
fn release_at_tail_jumps(e: IrExpr, name: &str, ty: &Type) -> IrExpr {
    match e {
        IrExpr::Let {
            name: n,
            value_ty,
            value,
            next,
        } => IrExpr::Let {
            name: n,
            value_ty,
            value,
            next: Box::new(release_at_tail_jumps(*next, name, ty)),
        },
        IrExpr::Release {
            name: n,
            ty: t,
            next,
        } => IrExpr::Release {
            name: n,
            ty: t,
            next: Box::new(release_at_tail_jumps(*next, name, ty)),
        },
        IrExpr::If {
            cond,
            then,
            els,
            ty: node_ty,
        } => IrExpr::If {
            cond,
            then: Box::new(release_at_tail_jumps(*then, name, ty)),
            els: Box::new(release_at_tail_jumps(*els, name, ty)),
            ty: node_ty,
        },
        IrExpr::Match {
            scrutinee,
            arms,
            ty: node_ty,
        } => IrExpr::Match {
            scrutinee,
            arms: arms
                .into_iter()
                .map(|arm| IrArm {
                    pattern: arm.pattern,
                    guard: arm.guard,
                    body: release_at_tail_jumps(arm.body, name, ty),
                })
                .collect(),
            ty: node_ty,
        },
        IrExpr::Try {
            body,
            arms,
            ty: node_ty,
        } => IrExpr::Try {
            body,
            arms: arms
                .into_iter()
                .map(|arm| IrArm {
                    pattern: arm.pattern,
                    guard: arm.guard,
                    body: release_at_tail_jumps(arm.body, name, ty),
                })
                .collect(),
            ty: node_ty,
        },
        jump @ (IrExpr::TailSelfCall { .. } | IrExpr::Throw { .. }) => IrExpr::Release {
            name: name.to_string(),
            ty: ty.clone(),
            next: Box::new(jump),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrFunction, IrProgram};
    use crate::ir_walk::walk;
    use crate::types::{EffectRow, FunctionType};

    fn string_fn(params: Vec<Type>) -> FunctionType {
        FunctionType {
            params,
            ret: Box::new(Type::String),
            throws: None,
            effects: EffectRow::default(),
        }
    }

    fn var(name: &str, ty: Type) -> IrExpr {
        IrExpr::Var {
            name: name.into(),
            ty,
        }
    }

    fn program(params: Vec<(&str, Type)>, ret: Type, body: IrExpr) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "f".into(),
                params: params
                    .into_iter()
                    .map(|(name, ty)| IrParam {
                        name: name.into(),
                        ty,
                    })
                    .collect(),
                ret,
                throws: None,
                effects: EffectRow::default(),
                body,
            }],
        }
    }

    fn count_retains(e: &IrExpr) -> usize {
        let mut n = 0;
        walk(e, &mut |x| {
            if matches!(x, IrExpr::Retain { .. }) {
                n += 1;
            }
        });
        n
    }

    fn releases(e: &IrExpr) -> Vec<String> {
        let mut names = Vec::new();
        walk(e, &mut |x| {
            if let IrExpr::Release { name, .. } = x {
                names.push(name.clone());
            }
        });
        names
    }

    #[test]
    fn call_args_are_retained_and_params_released() {
        // fn f(s: String) -> String { g(s) }
        let body = IrExpr::Call {
            callee: Box::new(IrExpr::FunctionRef {
                name: "g".into(),
                sig: string_fn(vec![Type::String]),
            }),
            args: vec![var("s", Type::String)],
            ret: Type::String,
        };
        let mut p = program(vec![("s", Type::String)], Type::String, body);
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        assert_eq!(count_retains(body), 1, "the call argument is retained");
        assert_eq!(releases(body), vec!["s"], "the param releases at the tail");
        // The tail value is preserved through a transfer temp.
        let IrExpr::Let { value, next, .. } = body else {
            panic!("expected a transfer let, got {body:?}");
        };
        assert!(matches!(value.as_ref(), IrExpr::Call { .. }));
        assert!(matches!(next.as_ref(), IrExpr::Release { .. }));
    }

    #[test]
    fn intrinsic_args_are_borrowed() {
        // fn f(s: String) -> Int { string_length(s) }
        let body = IrExpr::Intrinsic {
            name: "string_length".into(),
            args: vec![var("s", Type::String)],
            ret: Type::Int,
        };
        let mut p = program(vec![("s", Type::String)], Type::Int, body);
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        assert_eq!(count_retains(body), 0, "a borrowed Var is not retained");
        assert_eq!(releases(body), vec!["s"]);
    }

    #[test]
    fn array_push_element_is_owned() {
        // fn f(a: Array<String>, s: String) -> Array<String> { array_push(a, s) }
        let arr = Type::Array(Box::new(Type::String));
        let body = IrExpr::Intrinsic {
            name: "array_push".into(),
            args: vec![var("a", arr.clone()), var("s", Type::String)],
            ret: arr.clone(),
        };
        let mut p = program(
            vec![("a", arr.clone()), ("s", Type::String)],
            arr.clone(),
            body,
        );
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        assert_eq!(count_retains(body), 1, "only the element arg is retained");
        let mut rel = releases(body);
        rel.sort();
        assert_eq!(rel, vec!["a", "s"]);
    }

    #[test]
    fn nested_concat_temporary_is_released() {
        // fn f(a: String, b: String, c: String) -> String { (a ++ b) ++ c }
        let body = IrExpr::Concat {
            left: Box::new(IrExpr::Concat {
                left: Box::new(var("a", Type::String)),
                right: Box::new(var("b", Type::String)),
            }),
            right: Box::new(var("c", Type::String)),
        };
        let mut p = program(
            vec![
                ("a", Type::String),
                ("b", Type::String),
                ("c", Type::String),
            ],
            Type::String,
            body,
        );
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        // The inner concat is hoisted to a $rc temp and released after the outer.
        let rel = releases(body);
        assert!(
            rel.iter().any(|n| n.starts_with("$rc")),
            "inner concat temp released: {rel:?}"
        );
        assert_eq!(rel.len(), 4, "temp + three params: {rel:?}");
        assert_eq!(count_retains(body), 0, "concat borrows its operands");
    }

    #[test]
    fn if_releases_in_both_branches() {
        // fn f(s: String, c: Bool) -> String { if c { s } else { "x" } }
        let body = IrExpr::If {
            cond: Box::new(var("c", Type::Bool)),
            then: Box::new(var("s", Type::String)),
            els: Box::new(IrExpr::String("x".into())),
            ty: Type::String,
        };
        let mut p = program(
            vec![("s", Type::String), ("c", Type::Bool)],
            Type::String,
            body,
        );
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        assert_eq!(
            releases(body),
            vec!["s", "s"],
            "each branch releases the param once"
        );
        assert_eq!(count_retains(body), 1, "only the then-branch returns s");
    }

    #[test]
    fn match_scrutinee_temp_released_in_each_arm_and_bindings_borrow() {
        // fn f() -> String { match g() { Some(x) -> x, None -> "d" } }
        let opt = Type::Option(Box::new(Type::String));
        let body = IrExpr::Match {
            scrutinee: Box::new(IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef {
                    name: "g".into(),
                    sig: FunctionType {
                        params: vec![],
                        ret: Box::new(opt.clone()),
                        throws: None,
                        effects: EffectRow::default(),
                    },
                }),
                args: vec![],
                ret: opt.clone(),
            }),
            arms: vec![
                IrArm {
                    pattern: IrPattern::Variant {
                        variant: "Some".into(),
                        tag: 0,
                        bindings: vec![Some(("x".into(), Type::String))],
                    },
                    guard: None,
                    body: var("x", Type::String),
                },
                IrArm {
                    pattern: IrPattern::Wildcard { binding: None },
                    guard: None,
                    body: IrExpr::String("d".into()),
                },
            ],
            ty: Type::String,
        };
        let mut p = program(vec![], Type::String, body);
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        let rel = releases(body);
        assert_eq!(
            rel.len(),
            2,
            "scrutinee temp released once per arm: {rel:?}"
        );
        assert!(rel.iter().all(|n| n.starts_with("$rc")), "{rel:?}");
        assert_eq!(
            count_retains(body),
            1,
            "the payload binding is retained only when returned"
        );
    }

    #[test]
    fn tail_self_call_releases_before_the_jump() {
        // fn f(s: String, n: Int) -> Unit { tail_self_call(g(s), n - 1) }
        let body = IrExpr::TailSelfCall {
            args: vec![
                IrExpr::Call {
                    callee: Box::new(IrExpr::FunctionRef {
                        name: "g".into(),
                        sig: string_fn(vec![Type::String]),
                    }),
                    args: vec![var("s", Type::String)],
                    ret: Type::String,
                },
                IrExpr::Binary {
                    op: crate::types::BinaryOp::Sub,
                    ty: Type::Int,
                    left: Box::new(var("n", Type::Int)),
                    right: Box::new(IrExpr::Int(1)),
                },
            ],
            ty: Type::Unit,
        };
        let mut p = program(
            vec![("s", Type::String), ("n", Type::Int)],
            Type::Unit,
            body,
        );
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        assert_eq!(releases(body), vec!["s"]);
        // Structure: let $rc0 = call; let $rc1 = n - 1; release s; jump.
        fn find_jump_context(e: &IrExpr) -> bool {
            match e {
                IrExpr::Let { next, .. } => find_jump_context(next),
                IrExpr::Release { next, .. } => {
                    matches!(next.as_ref(), IrExpr::TailSelfCall { .. })
                }
                _ => false,
            }
        }
        assert!(
            find_jump_context(body),
            "release sits directly above the jump: {body:?}"
        );
        // All tail-call args are vars after atomization.
        let mut ok = true;
        walk(body, &mut |e| {
            if let IrExpr::TailSelfCall { args, .. } = e {
                ok &= args
                    .iter()
                    .all(|a| matches!(a, IrExpr::Var { .. } | IrExpr::Int(_)));
            }
        });
        assert!(ok, "tail-call args are atomized");
    }

    #[test]
    fn shadowed_bindings_are_renamed_apart() {
        // fn f(s: String) -> String { let s = g(s); s }
        let body = IrExpr::Let {
            name: "s".into(),
            value_ty: Type::String,
            value: Box::new(IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef {
                    name: "g".into(),
                    sig: string_fn(vec![Type::String]),
                }),
                args: vec![var("s", Type::String)],
                ret: Type::String,
            }),
            next: Box::new(var("s", Type::String)),
        };
        let mut p = program(vec![("s", Type::String)], Type::String, body);
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        let mut rel = releases(body);
        rel.sort();
        assert_eq!(
            rel,
            vec!["s", "s#1"],
            "outer param and shadowing let release separately"
        );
    }

    #[test]
    fn throw_atomizes_and_releases_before_the_raise() {
        // fn f(s: String) -> Int throws E { let x = g(s); throw MkE(x) }
        let e_ty = Type::Enum("E".into(), vec![]);
        let body = IrExpr::Let {
            name: "x".into(),
            value_ty: Type::String,
            value: Box::new(IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef {
                    name: "g".into(),
                    sig: string_fn(vec![Type::String]),
                }),
                args: vec![var("s", Type::String)],
                ret: Type::String,
            }),
            next: Box::new(IrExpr::Throw {
                value: Box::new(IrExpr::EnumValue {
                    ty: e_ty.clone(),
                    variant: "MkE".into(),
                    tag: 0,
                    payload: vec![var("x", Type::String)],
                }),
            }),
        };
        let mut p = program(vec![("s", Type::String)], Type::Int, body);
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        let rel = releases(body);
        assert!(
            rel.contains(&"x".to_string()) && rel.contains(&"s".to_string()),
            "both bindings release on the throw path: {rel:?}"
        );
        // The throw's operand is a var (atomized), so the releases above it
        // cannot invalidate an unevaluated operand.
        walk(body, &mut |e| {
            if let IrExpr::Throw { value } = e {
                assert!(matches!(value.as_ref(), IrExpr::Var { .. }));
            }
        });
    }

    #[test]
    fn lambda_bodies_release_their_own_params() {
        // fn f() -> Function { fn(s: String) -> String { s } }
        let lambda = IrExpr::Fn {
            params: vec![IrParam {
                name: "s".into(),
                ty: Type::String,
            }],
            ret: Type::String,
            throws: None,
            effects: EffectRow::default(),
            captures: vec![],
            body: Box::new(var("s", Type::String)),
        };
        let mut p = program(vec![], Type::OpaqueFunction, lambda);
        insert_rc_ops(&mut p);
        let body = &p.functions[0].body;
        let IrExpr::Fn { body: inner, .. } = body else {
            panic!("expected the lambda at the tail");
        };
        assert_eq!(releases(inner), vec!["s"]);
        assert_eq!(count_retains(inner), 1, "the returned param is retained");
    }
}
