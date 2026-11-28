//! The emitter: Heap IR (stage `RefCounted`) to C.
//!
//! The Heap IR already says everything the C needs: the evaluation order (ANF), where control
//! goes (structured statements, failure and defect landings), and when references are added and
//! given up (`dup` and `drop`). The emitter follows it mechanically:
//!
//! - Every IR variable is a C local (`v12`) declared at the top of its function. Values that
//!   leave a compound statement through `yield` or `break` are assigned to its destination,
//!   followed by a `goto` to the label after the statement; `recur` assigns the loop variables
//!   and jumps to the loop head.
//! - A call's defect landing is `if (em_defect != NULL) { … }` after the call, and its failure
//!   landing `if (em_failure != NULL) { … }`. `unwind` and `fail` return a dummy value.
//! - Heap objects get descriptors (`em_d3`) per shape: the constructor tag, the enum of an error
//!   value, and which slots hold references. Constructors without fields, string literals and
//!   closures without captures are static objects, which reference counting skips.
//! - A record or a tuple whose fields are all 8-byte scalars (or such aggregates) is held by
//!   value, as a C struct: [`crate::layout`] decides which types are, and the emitter holds every
//!   value of a type in that form. It boxes the value where the Heap IR makes it a reference (a
//!   list element, a closure environment slot, a field of an enum value, `Any`), so the IR's
//!   `Repr::Ref` and its `dup` and `drop` still describe the program.
//!
//! [`CTy`] is the C type of a value: a scalar by representation, or the struct of a layout.

use emela_core::{Builtin, EnumId, FiberOp, HostModule, HostRef, Type, TypeId};
use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, Module, OnFail, Operand, PrimOp, PrimTy, Repr,
    RuntimeOp, SeqId, Stage, Stmt, StmtId, Suspend, SwitchOn, Term, Value, VarId, enum_repr,
    repr_of, repr_of_subst,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::check::display_name;
use crate::layout::Values;
use crate::names;
use crate::writer::{Writer, call, init_list, param, signature};
use crate::{CError, CErrorKind, COutput, RUNTIME_H_FILE};

mod builtin;
mod entry;
mod external;
mod value;

/// What the generated `main` runs.
pub(crate) enum Run {
    /// The entry's `main` (its monomorphized copy), if any, and the exported fns (their copies)
    /// with their host names (`[target.native.export]`).
    Main(Option<FnId>, Vec<(FnId, Vec<String>)>),
    /// Test functions, with their names.
    Tests(Vec<(String, FnId)>),
}

impl Run {
    /// The functions the output calls first: `main`, then the exports, or the tests in order.
    pub(crate) fn roots(&self) -> Vec<FnId> {
        match self {
            Run::Main(main, exports) => main
                .iter()
                .copied()
                .chain(exports.iter().map(|(id, _)| *id))
                .collect(),
            Run::Tests(tests) => tests.iter().map(|&(_, id)| id).collect(),
        }
    }

    /// Replaces the roots with their indices after
    /// [`prune_functions`](emela_lir::analysis::reach::prune_functions), in the same order as
    /// [`Run::roots`] returned them.
    pub(crate) fn set_roots(&mut self, roots: &[FnId]) {
        let mut roots = roots.iter().copied();
        match self {
            Run::Main(main, exports) => {
                if main.is_some() {
                    *main = roots.next();
                }
                for (id, _) in exports {
                    *id = roots.next().expect("an export is a root");
                }
            }
            Run::Tests(tests) => {
                for (_, id) in tests {
                    *id = roots.next().expect("a test is a root");
                }
            }
        }
    }
}

pub(crate) fn emit(m: &Module, entry: &Run, values: bool) -> Result<COutput, Vec<CError>> {
    assert_eq!(
        m.stage,
        Stage::RefCounted,
        "the C emitter reads the Heap IR"
    );
    let mut e = Emitter {
        m,
        fn_names: m
            .functions
            .iter()
            .map(|(id, f)| names::function(ix(id), &f.name))
            .collect(),
        values: if values {
            Values::new(m)
        } else {
            Values::none(m)
        },
        descs: FxHashMap::default(),
        array_descs: FxHashMap::default(),
        lits: FxHashMap::default(),
        nullary: FxHashMap::default(),
        closures: FxHashMap::default(),
        statics: Writer::new(),
        types: Writer::new(),
        host_protos: Writer::new(),
        host_names: FxHashSet::default(),
        bundled: Vec::new(),
        host_files: Vec::new(),
        consts: Vec::new(),
        error_shows: m.error_shows.iter().copied().collect(),
        errors: Vec::new(),
    };
    e.values.declare(&mut e.types);
    let mut protos = Writer::new();
    let mut bodies = Writer::new();
    for (id, f) in m.functions.iter() {
        protos.static_proto(&e.signature(id, f, if f.constant { "_init" } else { "" }));
        if f.constant {
            protos.static_proto(&e.signature(id, f, ""));
        }
        if is_machine(f) {
            let name = &e.fn_names[ix(id)];
            protos.static_proto(&e.machine_signature(id, f));
            protos.static_proto(&signature(
                "em_word",
                &format!("{name}_resume"),
                ["em_ref", "em_word"],
            ));
        }
    }
    for (id, f) in m.functions.iter() {
        if let Some(ext) = &f.external {
            e.external(&mut bodies, id, f, ext);
        } else {
            e.function(&mut bodies, id, f);
        }
        bodies.blank();
    }
    match entry {
        Run::Main(main, exports) => {
            e.c_exports(&mut bodies, exports);
            e.c_main(&mut bodies, *main);
        }
        Run::Tests(tests) => e.c_test_main(&mut bodies, tests),
    }
    if !e.errors.is_empty() {
        return Err(e.errors);
    }
    let mut out = Writer::new();
    out.comment("Generated by Emela.");
    out.include(RUNTIME_H_FILE);
    out.blank();
    out.append(std::mem::take(&mut e.types));
    if !e.host_protos.is_empty() {
        out.comment("Host functions (spec 12.4).");
        out.append(std::mem::take(&mut e.host_protos));
        out.blank();
    }
    out.append(protos);
    out.blank();
    out.append(std::mem::take(&mut e.statics));
    out.blank();
    out.append(bodies);
    let bundled = m
        .core
        .bundled_files
        .iter()
        .filter(|file| e.bundled.contains(&file.path))
        .map(|file| (file.path.clone(), file.contents.clone()))
        .collect();
    Ok(COutput {
        main: out.finish(),
        bundled,
        host_files: e.host_files,
    })
}

fn ix<T>(id: la_arena::Idx<T>) -> usize {
    u32::from(id.into_raw()) as usize
}

/// The shape of a heap object made by the program.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DescKey {
    tag: u32,
    /// The enum index of an error value, or -1.
    enum_id: i32,
    name: String,
    show: Option<FnId>,
    refs: Vec<bool>,
    /// The size of a value held by value, whose box this is; 0 for any other object.
    value_size: u32,
}

struct Emitter<'m> {
    m: &'m Module,
    fn_names: Vec<String>,
    /// The types that are held by value, and their C structs.
    values: Values<'m>,
    descs: FxHashMap<DescKey, usize>,
    /// The descriptor of an array of a value layout, by layout.
    array_descs: FxHashMap<usize, usize>,
    lits: FxHashMap<Vec<u8>, usize>,
    /// Static objects of constructors without fields, by descriptor.
    nullary: FxHashMap<usize, usize>,
    /// Static closure records without captures, by function.
    closures: FxHashMap<FnId, usize>,
    /// Descriptors, literals and other static data, in order of first use.
    statics: Writer,
    /// The declarations of the struct types, before every use.
    types: Writer,
    host_protos: Writer,
    host_names: FxHashSet<String>,
    bundled: Vec<String>,
    host_files: Vec<String>,
    /// The const functions, for the cleanup at exit.
    consts: Vec<FnId>,
    /// The `show` of each error enum's descriptor (`emela_lir::Module::error_shows`), which shows a
    /// failure that reaches the entry.
    error_shows: FxHashMap<EnumId, FnId>,
    errors: Vec<CError>,
}

/// Where a `yield` goes: the destination of the enclosing statement and the label after it.
#[derive(Clone)]
struct Yield {
    dst: Option<VarId>,
    end: String,
    /// Whether the end of the sequence is right before the label, so that its final `yield`
    /// needs no `goto`.
    falls: bool,
}

impl Yield {
    fn new(dst: Option<VarId>, end: String) -> Self {
        Yield {
            dst,
            end,
            falls: false,
        }
    }

    fn falling(&self) -> Self {
        Yield {
            falls: true,
            ..self.clone()
        }
    }
}

/// The function being emitted.
struct Cx<'a> {
    f: &'a Function,
    /// What `unwind` and `fail` return in this function.
    dummy: Option<String>,
    /// The body, one level in.
    w: Writer,
    /// Blocks by label: their destination and end label.
    labels: FxHashMap<LabelId, Yield>,
    /// The enclosing loops, innermost last: their variables and head label.
    loops: Vec<(Vec<VarId>, String)>,
    used_labels: FxHashSet<String>,
    /// For a suspending function emitted as a state machine, its C name (its resume function
    /// is `name_resume`).
    machine: Option<String>,
    /// The resume points met so far.
    resumes: Vec<ResumePoint>,
}

/// A resume point of a state machine: what the frame of `state` restores, and where the input
/// goes.
struct ResumePoint {
    state: u32,
    saved: Vec<VarId>,
    dst: Option<VarId>,
}

/// Whether the function is emitted as a state machine: a suspending function with suspension
/// points.
fn is_machine(f: &Function) -> bool {
    f.suspends
        && f.body.is_some()
        && f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    on_suspend: Some(_),
                    ..
                }
            )
        })
}

/// The entry of a state machine with a frame: restores the saved variables of the frame's state,
/// frees the frame, takes the input as the suspended value's result and jumps after it.
fn resume_dispatch(w: &mut Writer, f: &Function, resumes: &[ResumePoint]) {
    w.if_("em_fr != NULL", |w| {
        w.switch("em_fr->slots[1].i", |w| {
            for r in resumes {
                w.case(&r.state.to_string(), |w| {
                    for (i, &v) in r.saved.iter().enumerate() {
                        w.assign(&var(v), &format!("em_fr->{}", slot(i + 3, f.vars[v].repr)));
                    }
                    w.stmt("em_frame_free(em_fr)");
                    if let Some(d) = r.dst {
                        w.assign(&var(d), &format!("em_in.{}", acc(f.vars[d].repr)));
                    }
                    w.goto(&format!("em_r{}", r.state));
                });
            }
            w.default_case(|w| w.stmt("em_unreachable()"));
        });
    });
}

impl Cx<'_> {
    fn goto(&mut self, label: &str) {
        self.used_labels.insert(label.to_owned());
        self.w.goto(label);
    }

    /// The label line, if anything jumps to it.
    fn label(&mut self, label: &str) {
        if self.used_labels.contains(label) {
            self.w.label(label);
        }
    }

    /// What `unwind` and `fail` return.
    fn dummy_return(&mut self) {
        self.w.ret(self.dummy.as_deref());
    }
}

/// The C type of a value: a scalar by representation, or the struct of a layout
/// ([`crate::layout`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CTy {
    Scalar(Repr),
    Agg(usize),
}

impl CTy {
    fn is_void(self) -> bool {
        self == CTy::Scalar(Repr::Void)
    }

    fn repr(self) -> Repr {
        match self {
            CTy::Scalar(r) => r,
            CTy::Agg(_) => Repr::Ref,
        }
    }
}

/// The value `unwind`, `fail` and failed conversions return in a function returning `r`.
fn dummy(t: CTy, values: &Values<'_>) -> Option<String> {
    match t {
        CTy::Scalar(Repr::Void) => None,
        CTy::Scalar(_) => Some("0".to_owned()),
        CTy::Agg(i) => Some(format!("({}){{0}}", values.get(i).c_ty)),
    }
}

/// `slots[i].a`: the member of a slot holding a value of the representation.
fn slot(i: usize, r: Repr) -> String {
    format!("slots[{i}].{}", acc(r))
}

/// `(em_word){.a = value}`: a word holding a value of the representation.
fn word_of(r: Repr, value: &str) -> String {
    format!("(em_word){{.{} = {value}}}", acc(r))
}

fn var(v: VarId) -> String {
    format!("v{}", ix(v))
}

pub(crate) fn cty(r: Repr) -> &'static str {
    match r {
        Repr::I32 => "int32_t",
        Repr::I64 => "int64_t",
        Repr::F64 => "double",
        Repr::Ref => "em_ref",
        Repr::Void => "void",
    }
}

/// The member of `em_word` holding a value of the representation.
fn acc(r: Repr) -> &'static str {
    match r {
        Repr::I32 => "i",
        Repr::I64 => "l",
        Repr::F64 => "f",
        Repr::Ref => "p",
        Repr::Void => "l",
    }
}

fn stmt_label(s: StmtId, what: &str) -> String {
    format!("s{}_{what}", ix(s))
}

impl<'m> Emitter<'m> {
    fn core(&self) -> &'m emela_core::Module {
        &self.m.core
    }

    fn repr(&self, ty: TypeId) -> Repr {
        repr_of(self.core(), ty)
    }

    /// The C type of a value of the Core type `ty`: the struct of a value layout, or the scalar
    /// of its representation ([`crate::layout`]).
    fn ct(&self, ty: TypeId) -> CTy {
        match self.values.index(ty) {
            Some(i) => CTy::Agg(i),
            None => CTy::Scalar(repr_of(self.core(), ty)),
        }
    }

    /// The C type of `ct`.
    fn cty(&self, ct: CTy) -> &str {
        match ct {
            CTy::Scalar(r) => cty(r),
            CTy::Agg(i) => &self.values.get(i).c_ty,
        }
    }

    /// [`Emitter::cty`] of a Core type.
    fn ty_cty(&self, ty: TypeId) -> &str {
        self.cty(self.ct(ty))
    }

    /// An initializer of that type that is readable without a value: `{0, 0, …}` for a struct.
    fn zero(&self, ct: CTy) -> String {
        match ct {
            CTy::Agg(i) => self.values.get(i).init(std::iter::empty()),
            CTy::Scalar(_) => "0".to_owned(),
        }
    }

    /// The boxed form of a value of layout `i` (`em_box_value`): a heap object whose slots are the
    /// bytes of the value, for the places where the IR wants a reference.
    fn box_of(&mut self, value: &str, i: usize) -> String {
        let desc = self.layout_desc(i);
        call(
            "em_box_value",
            [format!("&{value}"), format!("&em_d{desc}")],
        )
    }

    /// The descriptor of a box of layout `i` (the shape it would have as a heap object).
    fn layout_desc(&mut self, i: usize) -> usize {
        let (tag, name, n) = {
            let l = self.values.get(i);
            (l.tag, l.name.clone(), l.slots())
        };
        let value_size = self.values.get(i).size as u32;
        self.desc(DescKey {
            tag,
            enum_id: -1,
            name,
            show: None,
            refs: vec![false; n],
            value_size,
        })
    }

    /// The descriptor of an array whose element is layout `i`.
    fn array_desc(&mut self, i: usize) -> usize {
        if let Some(&n) = self.array_descs.get(&i) {
            return n;
        }
        let n = self.array_descs.len();
        let size = self.values.get(i).size;
        self.statics
            .decl("static const uint8_t", &format!("em_ar{n}[]"), Some("{0}"));
        self.statics.decl(
            "static const em_desc",
            &format!("em_ad{n}"),
            Some(&init_list([
                "EM_KIND_ARRAY".to_owned(),
                "0".to_owned(),
                "0".to_owned(),
                "-1".to_owned(),
                "\"Array\"".to_owned(),
                "NULL".to_owned(),
                format!("em_ar{n}"),
                size.to_string(),
            ])),
        );
        self.array_descs.insert(i, n);
        n
    }

    fn internal(&mut self, message: impl Into<String>) {
        self.errors.push(CError::new(
            CErrorKind::Internal,
            format!("internal error in the C emitter: {}", message.into()),
        ));
    }

    /// `RET name(PARAMS)`.
    fn signature(&self, id: FnId, f: &Function, suffix: &str) -> String {
        let params = f
            .params
            .iter()
            .map(|&p| param(self.ty_cty(f.vars[p].ty), &var(p)));
        signature(
            self.ty_cty(f.ret),
            &format!("{}{suffix}", self.fn_names[ix(id)]),
            params,
        )
    }

    fn operand(&mut self, o: &Operand) -> String {
        match o {
            Operand::Var(v) => var(*v),
            Operand::Const(c) => self.constant(c),
        }
    }

    /// A `switch` key as C: a variant number or a literal.
    fn case_key(&mut self, key: &CaseKey) -> String {
        match key {
            CaseKey::Variant(k) => k.to_string(),
            CaseKey::Lit(c) => self.constant(c),
        }
    }

    fn constant(&mut self, c: &Const) -> String {
        match c {
            Const::Int(n) => names::int32(*n),
            Const::Int64(n) => names::int64(*n),
            Const::Float(x) => names::float(*x),
            Const::Bool(b) => if *b { "1" } else { "0" }.to_owned(),
            Const::String(s) => self.literal(s.as_bytes()),
            Const::Unit => "0".to_owned(),
        }
    }

    fn operand_repr(f: &Function, o: &Operand) -> Repr {
        match o {
            Operand::Var(v) => f.vars[*v].repr,
            Operand::Const(c) => c.repr(),
        }
    }

    /// The Core type of an operand (the type of a constant is its own).
    fn operand_ty(f: &Function, o: &Operand) -> TypeId {
        match o {
            Operand::Var(v) => f.vars[*v].ty,
            Operand::Const(c) => match c {
                Const::Int(_) => TypeId::INT,
                Const::Int64(_) => TypeId::INT64,
                Const::Float(_) => TypeId::FLOAT,
                Const::Bool(_) => TypeId::BOOL,
                Const::String(_) => TypeId::STRING,
                Const::Unit => TypeId::UNIT,
            },
        }
    }

    /// A static string object.
    fn literal(&mut self, bytes: &[u8]) -> String {
        let n = match self.lits.get(bytes) {
            Some(&n) => n,
            None => {
                let n = self.lits.len();
                self.lits.insert(bytes.to_vec(), n);
                self.statics.decl(
                    &format!(
                        "static struct {{ em_hdr hdr; int64_t len; char bytes[{}]; }}",
                        bytes.len() + 1
                    ),
                    &format!("em_s{n}"),
                    Some(&init_list([
                        "EM_STATIC_HDR(&em_desc_string)".to_owned(),
                        bytes.len().to_string(),
                        names::string_literal(bytes),
                    ])),
                );
                n
            }
        };
        format!("((em_ref)&em_s{n})")
    }

    /// The descriptor's C name.
    fn desc(&mut self, key: DescKey) -> usize {
        if let Some(&n) = self.descs.get(&key) {
            return n;
        }
        let n = self.descs.len();
        let refs = if key.refs.is_empty() {
            "NULL".to_owned()
        } else {
            let flags = key.refs.iter().map(|&r| if r { "1" } else { "0" });
            self.statics.decl(
                "static const uint8_t",
                &format!("em_r{n}[]"),
                Some(&init_list(flags)),
            );
            format!("em_r{n}")
        };
        let show = match key.show {
            Some(f) => self.fn_names[ix(f)].clone(),
            None => "NULL".to_owned(),
        };
        let mut init = vec![
            "EM_KIND_FIELDS".to_owned(),
            key.refs.len().to_string(),
            key.tag.to_string(),
            key.enum_id.to_string(),
            names::string_literal(key.name.as_bytes()),
            show,
            refs,
        ];
        // Only the box of a value has a size to give (`fin` stays 0, as it is for the shapes the
        // program makes); every other descriptor keeps the shorter form.
        if key.value_size != 0 {
            init.push(key.value_size.to_string());
        }
        self.statics.decl(
            "static const em_desc",
            &format!("em_d{n}"),
            Some(&init_list(init)),
        );
        self.descs.insert(key, n);
        n
    }

    fn function(&mut self, out: &mut Writer, id: FnId, f: &Function) {
        let Some(body) = f.body else {
            self.internal(format!("fn {} has no body", f.name));
            return;
        };
        let ret = self.ct(f.ret);
        let machine = is_machine(f).then(|| self.fn_names[ix(id)].clone());
        let mut cx = Cx {
            f,
            dummy: dummy(ret, &self.values),
            w: Writer::indented(1),
            labels: FxHashMap::default(),
            loops: Vec::new(),
            used_labels: FxHashSet::default(),
            machine: machine.clone(),
            resumes: Vec::new(),
        };
        self.seq(&mut cx, body, None);
        let sig = match &machine {
            Some(_) => self.machine_signature(id, f),
            None => self.signature(id, f, if f.constant { "_init" } else { "" }),
        };
        let locals: Vec<(String, String, String)> = f
            .vars
            .iter()
            .filter(|(v, _)| !f.params.contains(v))
            .map(|(v, data)| {
                (
                    self.ty_cty(data.ty).to_owned(),
                    var(v),
                    self.zero(self.ct(data.ty)),
                )
            })
            .collect();
        out.comment(&f.name);
        out.static_func(&sig, |w| {
            for (ct, v, init) in &locals {
                w.decl(ct, v, Some(init));
            }
            if machine.is_some() {
                resume_dispatch(w, f, &cx.resumes);
            }
            w.append(cx.w);
        });
        if let Some(name) = &machine {
            self.machine_wrappers(out, id, f, name, ret);
        }
        if f.constant {
            self.const_wrapper(out, id, f, ret);
        }
    }

    /// `RET name_sm(em_ref em_fr, em_word em_in, PARAMS)`: the body of a suspending function,
    /// entered without a frame by the function itself and with one by the driver loop.
    fn machine_signature(&self, id: FnId, f: &Function) -> String {
        let mut params = vec![param("em_ref", "em_fr"), param("em_word", "em_in")];
        params.extend(
            f.params
                .iter()
                .map(|&p| param(self.ty_cty(f.vars[p].ty), &var(p))),
        );
        signature(
            self.ty_cty(f.ret),
            &format!("{}_sm", self.fn_names[ix(id)]),
            params,
        )
    }

    /// The function itself (entering the state machine without a frame) and its resume function
    /// for the driver loop (entering it with a frame and the input, returning the result as a
    /// word).
    fn machine_wrappers(&self, out: &mut Writer, id: FnId, f: &Function, name: &str, ret: CTy) {
        let machine = format!("{name}_sm");
        let mut args = vec!["NULL".to_owned(), "(em_word){.l = 0}".to_owned()];
        args.extend(f.params.iter().map(|&p| var(p)));
        let enter = call(&machine, &args);
        out.static_func(&self.signature(id, f, ""), |w| {
            if ret.is_void() {
                w.stmt(&enter);
            } else {
                w.ret(Some(&enter));
            }
        });
        let mut zeros = vec!["fr".to_owned(), "in".to_owned()];
        zeros.extend(f.params.iter().map(|&p| {
            let ct = self.ct(f.vars[p].ty);
            match ct {
                CTy::Scalar(Repr::Ref) => "NULL".to_owned(),
                ct => self.zero(ct),
            }
        }));
        let resume = call(&machine, &zeros);
        let sig = signature(
            "em_word",
            &format!("{name}_resume"),
            [param("em_ref", "fr"), param("em_word", "in")],
        );
        out.static_func(&sig, |w| {
            w.decl("em_word", "out", None);
            w.assign("out.l", "0");
            if ret.is_void() {
                w.stmt(&resume);
            } else {
                w.assign(&format!("out.{}", acc(ret.repr())), &resume);
            }
            w.ret(Some("out"));
        });
    }

    /// The function a const's callers call: evaluates the body once and remembers the value or
    /// the defect (spec 5.6).
    fn const_wrapper(&mut self, out: &mut Writer, id: FnId, f: &Function, ret: CTy) {
        let CTy::Scalar(ret) = ret else {
            // A type held by value is never the result of a const fn (see `layout::excluded`).
            self.internal(format!("the const fn {} returns a value", f.name));
            return;
        };
        let n = ix(id);
        let name = &self.fn_names[n];
        self.statics.decl(
            "static em_const",
            &format!("em_c{n}"),
            Some(&init_list([
                "0",
                "{0}",
                if ret == Repr::Ref { "true" } else { "false" },
                "NULL",
                names::string_literal(f.name.as_bytes()).as_str(),
            ])),
        );
        let cell = format!("&em_c{n}");
        let enter = format!("!{}", call("em_const_enter", [cell.as_str(), "&w"]));
        let leave = call("em_const_leave", [cell.as_str(), "w"]);
        let init = call(&format!("{name}_init"), [""; 0]);
        out.static_func(&self.signature(id, f, ""), |w| {
            w.decl("em_word", "w", None);
            if ret == Repr::Void {
                w.if_return(&enter, None);
                w.stmt(&init);
                w.assign("w.l", "0");
                w.stmt(&leave);
            } else {
                let a = acc(ret);
                w.if_return(&enter, Some(&format!("w.{a}")));
                w.decl(cty(ret), "v", Some(&init));
                w.assign("w.l", "0");
                w.assign(&format!("w.{a}"), "v");
                w.stmt(&leave);
                w.ret(Some("v"));
            }
        });
        self.consts.push(id);
    }

    fn seq(&mut self, cx: &mut Cx<'_>, id: SeqId, y: Option<&Yield>) {
        let f = cx.f;
        let seq = &f.seqs[id];
        for &s in &seq.stmts {
            self.stmt(cx, s);
        }
        self.term(cx, &seq.term, y);
    }

    fn term(&mut self, cx: &mut Cx<'_>, term: &Term, y: Option<&Yield>) {
        match term {
            Term::Yield(v) => {
                let Some(y) = y else {
                    self.internal(format!("yield without a target in {}", cx.f.name));
                    return;
                };
                if let (Some(dst), Some(v)) = (y.dst, v) {
                    let v = self.operand(v);
                    cx.w.assign(&var(dst), &v);
                }
                if !y.falls {
                    cx.goto(&y.end);
                }
            }
            Term::Return(v) => match v {
                Some(v) => {
                    let v = self.operand(v);
                    cx.w.ret(Some(&v));
                }
                None => cx.dummy_return(),
            },
            Term::Fail(v) => {
                let v = self.operand(v);
                cx.w.assign("em_failure", &v);
                cx.dummy_return();
            }
            Term::Break { label, value } => {
                let Some(target) = cx.labels.get(label).cloned() else {
                    self.internal(format!("break to an unknown label in {}", cx.f.name));
                    return;
                };
                if let (Some(dst), Some(v)) = (target.dst, value) {
                    let v = self.operand(v);
                    cx.w.assign(&var(dst), &v);
                }
                cx.goto(&target.end);
            }
            Term::Recur(args) => {
                let Some((vars, head)) = cx.loops.last().cloned() else {
                    self.internal(format!("recur outside a loop in {}", cx.f.name));
                    return;
                };
                // Parallel assignment: the new values may read the old ones.
                let values: Vec<String> = args.iter().map(|a| self.operand(a)).collect();
                cx.w.block_begin();
                for (i, &v) in vars.iter().enumerate() {
                    let ty = self.ty_cty(cx.f.vars[v].ty).to_owned();
                    cx.w.decl(&ty, &format!("t{i}"), Some(&values[i]));
                }
                for (i, &v) in vars.iter().enumerate() {
                    cx.w.assign(&var(v), &format!("t{i}"));
                }
                cx.goto(&head);
                cx.w.end();
            }
            Term::Unwind => cx.dummy_return(),
            Term::Unreachable => cx.w.stmt("em_unreachable()"),
            Term::Defect(..) => self.internal(format!("defect left in {}", cx.f.name)),
        }
    }

    /// Emits `body` into a separate writer (to decide afterwards whether a label before it is
    /// used).
    fn capture(&mut self, cx: &mut Cx<'_>, body: impl FnOnce(&mut Self, &mut Cx<'_>)) -> Writer {
        let aside = cx.w.aside();
        let saved = std::mem::replace(&mut cx.w, aside);
        body(self, cx);
        std::mem::replace(&mut cx.w, saved)
    }

    fn stmt(&mut self, cx: &mut Cx<'_>, id: StmtId) {
        let f = cx.f;
        match &f.stmts[id] {
            Stmt::Let {
                dst,
                value,
                on_defect,
                on_suspend,
            } => {
                // Suspending code called where nothing may suspend runs in place (spec 8.7).
                let sync = !f.suspends && emela_lir::may_suspend(value);
                if sync {
                    cx.w.stmt("em_sync_enter()");
                }
                match f.reuse.get(&id).copied() {
                    // Perceus reuse (C4/PR-042): write the new fields into the old block instead
                    // of allocating a new one.
                    Some(old) => self.reuse_make(cx, *dst, old, value),
                    None => self.value(cx, *dst, value),
                }
                if sync {
                    cx.w.stmt("em_sync_leave()");
                }
                if let Some(edge) = on_suspend {
                    self.suspend_edge(cx, edge, *dst);
                }
                if let Some(landing) = on_defect {
                    cx.w.if_begin("em_defect != NULL");
                    self.seq(cx, *landing, None);
                    cx.w.end();
                }
                let on_fail = match value {
                    Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } => on_fail,
                    _ => &None,
                };
                match on_fail {
                    Some(OnFail::Handle { error, body }) => {
                        cx.w.if_begin("em_failure != NULL");
                        cx.w.assign(&var(*error), "em_failure");
                        cx.w.assign("em_failure", "NULL");
                        self.seq(cx, *body, None);
                        cx.w.end();
                    }
                    Some(_) => self.internal(format!("Flow IR failure edge in {}", f.name)),
                    None => {}
                }
            }
            Stmt::If {
                dst,
                cond,
                then,
                else_,
            } => {
                let y = Yield::new(*dst, stmt_label(id, "end"));
                let cond = self.operand(cond);
                cx.w.if_begin(&cond);
                self.seq(cx, *then, Some(&y));
                cx.w.else_begin();
                self.seq(cx, *else_, Some(&y.falling()));
                cx.w.end();
                cx.label(&y.end);
            }
            Stmt::Switch {
                dst,
                on,
                cases,
                default,
            } => {
                let y = Yield::new(*dst, stmt_label(id, "end"));
                let scrutinee = match on {
                    // A value held by value has one constructor, so its tag is 0.
                    SwitchOn::Tag(v) if self.values.index(f.vars[*v].ty).is_some() => {
                        "0".to_owned()
                    }
                    SwitchOn::Tag(v) if f.vars[*v].repr == Repr::Ref => call("em_tag", [var(*v)]),
                    SwitchOn::Tag(v) | SwitchOn::Lit(v) => var(*v),
                };
                cx.w.switch_begin(&scrutinee);
                for case in cases {
                    // Several keys of one case share its body: C lists their labels one after
                    // the other before it.
                    for key in &case.keys[..case.keys.len() - 1] {
                        cx.w.case_label(&self.case_key(key));
                    }
                    let key = self.case_key(&case.keys[case.keys.len() - 1]);
                    cx.w.case_begin(&key);
                    self.seq(cx, case.body, Some(&y));
                    cx.w.end();
                }
                match default {
                    Some(d) => {
                        cx.w.default_begin();
                        self.seq(cx, *d, Some(&y));
                        cx.w.end();
                    }
                    None => cx.w.default_stmt("em_unreachable()"),
                }
                cx.w.switch_end();
                cx.label(&y.end);
            }
            Stmt::Loop {
                dst,
                vars,
                inits,
                body,
            } => {
                for (&v, init) in vars.iter().zip(inits) {
                    let init = self.operand(init);
                    cx.w.assign(&var(v), &init);
                }
                let y = Yield::new(*dst, stmt_label(id, "end")).falling();
                let head = stmt_label(id, "head");
                cx.loops.push((vars.clone(), head.clone()));
                let text = self.capture(cx, |e, cx| {
                    cx.w.block_begin();
                    e.seq(cx, *body, Some(&y));
                    cx.w.end();
                });
                cx.loops.pop();
                cx.label(&head);
                cx.w.append(text);
                cx.label(&y.end);
            }
            Stmt::Block { dst, label, body } => {
                let y = Yield::new(*dst, stmt_label(id, "end"));
                cx.labels.insert(*label, y.clone());
                cx.w.block_begin();
                self.seq(cx, *body, Some(&y.falling()));
                cx.w.end();
                cx.label(&y.end);
            }
            Stmt::Protect { .. } => self.internal(format!("protect left in {}", f.name)),
            Stmt::Dup(v) => self.count(cx, "em_dup", *v),
            Stmt::Drop(v) => {
                // Drop specialisation (C3/PR-027): a value whose payload holds no reference and
                // whose type has no finalizer gives its block back without walking it.
                let op = match cx.f.drop_kinds.get(&id) {
                    Some(emela_lir::opt::drop_spec::DropKind::Block) => "em_drop_block",
                    _ => "em_drop",
                };
                self.count(cx, op, *v);
            }
        }
    }

    /// A suspension edge: when the value came back suspended, the function pushes a frame with
    /// the state and the saved variables onto the continuation and returns. Its resume point,
    /// where the frame's entry jumps with the input, is right after (before the landings).
    fn suspend_edge(&mut self, cx: &mut Cx<'_>, edge: &Suspend, dst: Option<VarId>) {
        let f = cx.f;
        let Some(name) = cx.machine.clone() else {
            self.internal(format!(
                "a suspension edge outside a state machine in {}",
                f.name
            ));
            return;
        };
        let mut refs = vec![false, false, true];
        refs.extend(edge.saved.iter().map(|&v| f.vars[v].repr == Repr::Ref));
        let desc = self.desc(DescKey {
            tag: 0,
            enum_id: -1,
            name: display_name(&f.name).to_owned(),
            show: None,
            refs,
            value_size: 0,
        });
        cx.w.if_begin("em_suspending");
        let frame = call(
            "em_frame_new",
            [
                format!("&em_d{desc}"),
                format!("{name}_resume"),
                edge.state.to_string(),
                edge.saved.len().to_string(),
            ],
        );
        cx.w.decl("em_ref", "fr", Some(&frame));
        for (i, &v) in edge.saved.iter().enumerate() {
            let r = f.vars[v].repr;
            cx.w.assign(&format!("fr->slots[{}]", i + 3), &word_of(r, &var(v)));
        }
        cx.w.stmt("em_frame_push(fr)");
        cx.dummy_return();
        cx.w.end();
        cx.w.label_at_margin(&format!("em_r{}", edge.state));
        cx.resumes.push(ResumePoint {
            state: edge.state,
            saved: edge.saved.clone(),
            dst,
        });
    }

    /// `dst = <expression>;`, or the expression alone without a destination.
    fn assign(cx: &mut Cx<'_>, dst: Option<VarId>, expr: &str) {
        match dst {
            Some(d) => cx.w.assign(&var(d), expr),
            None => cx.w.stmt(expr),
        }
    }

    /// `(em_word){.i = x}` for a slot: a scalar, or a box of a value held by value.
    fn word(&mut self, f: &Function, o: &Operand) -> String {
        let ct = self.ct(Self::operand_ty(f, o));
        let v = self.operand(o);
        match ct {
            CTy::Scalar(r) => word_of(r, &v),
            CTy::Agg(i) => format!("(em_word){{.p = {}}}", self.box_of(&v, i)),
        }
    }

    /// `em_dup(v)` or `em_drop(v)`, unless the value is held by value: then it owns no reference
    /// and there is nothing to count.
    fn count(&mut self, cx: &mut Cx<'_>, op: &str, v: VarId) {
        if self.values.index(cx.f.vars[v].ty).is_none() {
            cx.w.stmt(&call(op, [var(v)]));
        }
    }

    /// Allocates an object of `desc` with `args` in its slots (consumed: the IR passes their
    /// references on).
    fn alloc(
        &mut self,
        cx: &mut Cx<'_>,
        dst: Option<VarId>,
        desc: usize,
        first: Option<String>,
        args: &[Operand],
    ) {
        let n = args.len() + usize::from(first.is_some());
        let target = match dst {
            Some(d) => var(d),
            None => {
                self.internal(format!("allocation without a destination in {}", cx.f.name));
                return;
            }
        };
        cx.w.assign(
            &target,
            &call("em_alloc", [format!("&em_d{desc}"), n.to_string()]),
        );
        let mut slot = 0;
        if let Some(first) = first {
            cx.w.assign(&format!("{target}->slots[0]"), &first);
            slot = 1;
        }
        for a in args {
            let w = self.word(cx.f, a);
            cx.w.assign(&format!("{target}->slots[{slot}]"), &w);
            slot += 1;
        }
    }

    /// Which slots of an object made from `args` hold references. Taken from the arguments'
    /// representations: after monomorphisation they are fixed per type.
    fn refs_of(f: &Function, args: &[Operand]) -> Vec<bool> {
        args.iter()
            .map(|a| Self::operand_repr(f, a) == Repr::Ref)
            .collect()
    }

    /// `dst = (struct){args}`: a record or tuple held by value, without an allocation.
    fn aggregate(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, i: usize, args: &[Operand]) {
        let Some(d) = dst else {
            self.internal(format!("aggregate without a destination in {}", cx.f.name));
            return;
        };
        let values: Vec<String> = args.iter().map(|o| self.operand(o)).collect();
        let init = self.values.get(i).init(values.into_iter());
        let c_ty = self.values.get(i).c_ty.clone();
        cx.w.assign(&var(d), &format!("({c_ty}){init}"));
    }

    /// The `Make` `dst = …` of a statement whose old block is reused (C4/PR-042): the destination
    /// becomes the old variable, every old field that holds a reference is released, and the new
    /// fields are written into the same object.
    ///
    /// `opt::reuse` proved that the old variable has exactly one use, the removed drop, so no
    /// argument reads the old block and no other variable reaches it. A reference field is
    /// released with the drop the old block's own release would have run, before the new field is
    /// stored: the slot owns one reference and the new argument owns another (the `Make` consumes
    /// it), so the release cannot free the argument's value, even when both denote the same
    /// object. The constructor and the type are the same, so the descriptor, the tag, the number
    /// of slots and the reference slots are the ones the object already has; a value held by
    /// value is a struct copy, exactly as without reuse (a nullary constructor is static data,
    /// which both values already are).
    fn reuse_make(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, old: VarId, value: &Value) {
        let Value::Make { ctor, args } = value else {
            self.internal(format!(
                "a reuse of a value that is not a `make` in {}",
                cx.f.name
            ));
            return;
        };
        let Some(d) = dst else {
            self.internal(format!("reuse without a destination in {}", cx.f.name));
            return;
        };
        let f = cx.f;
        if enum_repr(self.core(), ctor.enum_id) == Repr::I32 {
            // The representation is the tag itself: there is no block.
            cx.w.assign(&var(d), &var(old));
            return;
        }
        if let Some(i) = self.values.index(f.vars[d].ty) {
            // Held by value: no allocation to reuse; the copy is the whole statement.
            self.aggregate(cx, Some(d), i, args);
            return;
        }
        cx.w.assign(&var(d), &var(old));
        // The old slots own one reference each: give each back with the drop the removed `drop`
        // would have run, in slot order, before the slot is overwritten.
        for slot in self.ref_slots(f, old, ctor.enum_id, ctor.variant) {
            let slot = format!("{}->slots[{slot}].p", var(d));
            cx.w.stmt(&call("em_drop", [slot]));
        }
        for (slot, a) in args.iter().enumerate() {
            let w = self.word(f, a);
            cx.w.assign(&format!("{}->slots[{slot}]", var(d)), &w);
        }
    }

    /// The slots of the object of `old` that hold a reference, in slot order, from the fields of
    /// the constructor's variant and the type arguments of the object's type.
    ///
    /// The descriptor was made from the same field representations
    /// ([`Value::Make`] marks a slot a reference when its argument is `Repr::Ref`, and the
    /// verifier takes an argument's representation from the field type), so these are the slots
    /// the old block's drop would release.
    fn ref_slots(
        &mut self,
        f: &Function,
        old: VarId,
        enum_id: EnumId,
        variant: usize,
    ) -> Vec<usize> {
        let reprs = {
            let core = self.core();
            match core.ty(f.vars[old].ty) {
                Type::Enum(id, args) if *id == enum_id => {
                    core.enums[enum_id].variants.get(variant).map(|v| {
                        v.tys
                            .iter()
                            .map(|&t| repr_of_subst(core, t, args))
                            .collect::<Vec<_>>()
                    })
                }
                _ => None,
            }
        };
        let Some(reprs) = reprs else {
            self.internal(format!("a reuse of an unexpected value in {}", f.name));
            return Vec::new();
        };
        reprs
            .iter()
            .enumerate()
            .filter(|(_, r)| **r == Repr::Ref)
            .map(|(i, _)| i)
            .collect()
    }

    /// The representation of the first type argument of the enum type `ty` (`Exit[A, E]`,
    /// `Option[A]`), of the elements of the array type `ty`, or of the value of the function type
    /// `ty`.
    fn inner_repr(&self, ty: TypeId) -> Repr {
        match self.core().ty(ty) {
            Type::Enum(_, args) => args.first().map_or(Repr::Void, |&a| self.repr(a)),
            Type::Array(elem) => self.repr(*elem),
            Type::Fn(ft) => self.repr(ft.ret),
            _ => Repr::Ref,
        }
    }

    /// The layout of the elements of the array type `ty`, if they are held by value.
    fn elem_of(&self, f: &Function, o: &Operand) -> Option<usize> {
        let Type::Array(elem) = self.core().ty(Self::operand_ty(f, o)) else {
            return None;
        };
        self.values.index(*elem)
    }

    /// The `ref` argument of a word array read, which is told whether its elements are references
    /// instead of reading that from the array's descriptor (`emela_runtime.h`). Arrays are typed,
    /// so the element type of the variable settles it; `arr` is the rendered array operand, which
    /// is used only when it is not a variable. A read of an array whose elements are held by value
    /// does not come here: it takes the `_agg` path, which copies the element out.
    fn elem_refs(&self, f: &Function, args: &[Operand], arr: &str) -> String {
        match args.first() {
            Some(Operand::Var(v)) if self.inner_repr(f.vars[*v].ty) == Repr::Ref => {
                "true".to_owned()
            }
            Some(Operand::Var(_)) => "false".to_owned(),
            // There is no array constant, so this is not reached today; reading the descriptor in
            // the argument is what the runtime did before the flag existed.
            _ => format!("EM_ARRAY_REFS({arr})"),
        }
    }

    /// The descriptor of an array of `ty` (values are held inline; other elements are words,
    /// references or not).
    fn array_of(&mut self, ty: Option<TypeId>) -> String {
        let elem = ty.and_then(|ty| match self.core().ty(ty) {
            Type::Array(elem) => Some(*elem),
            _ => None,
        });
        if let Some(i) = elem.and_then(|e| self.values.index(e)) {
            let n = self.array_desc(i);
            return format!("&em_ad{n}");
        }
        if elem.is_some_and(|e| self.repr(e) == Repr::Ref) {
            "&em_desc_array_ref".to_owned()
        } else {
            "&em_desc_array_val".to_owned()
        }
    }

    /// The descriptor of a box of layout `i`, as a C expression.
    fn elem_desc(&mut self, i: usize) -> String {
        let n = self.layout_desc(i);
        format!("&em_d{n}")
    }

    /// A concurrency operation: a call of the runtime (spec chapter 11). The runtime moves values
    /// of any type, so it is told their representation.
    fn fiber(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, op: FiberOp, args: &[Operand]) {
        let f = cx.f;
        let a: Vec<String> = args.iter().map(|o| self.operand(o)).collect();
        let arg_ty = |i: usize| match args.get(i) {
            Some(Operand::Var(v)) => Some(f.vars[*v].ty),
            _ => None,
        };
        let dst_repr = dst.map_or(Repr::Void, |d| f.vars[d].repr);
        let dst_ty = dst.map(|d| f.vars[d].ty);
        // `name(a0, REPR)`: an operation told the representation of the value it moves.
        let op_call = |name: &str, repr: Repr| call(name, [a[0].as_str(), repr_code(repr)]);
        // The word an operation returns, unpacked into the destination.
        let word = |cx: &mut Cx<'_>, expr: String| match dst {
            Some(d) => {
                cx.w.assign(&var(d), &format!("{expr}.{}", acc(f.vars[d].repr)))
            }
            None => cx.w.stmt(&format!("(void){expr}")),
        };
        match op {
            FiberOp::Scope { background, .. } => {
                let repr = arg_ty(0).map_or(Repr::Void, |t| self.inner_repr(t));
                let flag = if background { "true" } else { "false" };
                word(
                    cx,
                    call("em_fiber_scope", [a[0].as_str(), repr_code(repr), flag]),
                );
            }
            FiberOp::Fork => Self::assign(cx, dst, &call("em_fiber_fork", &a[..2])),
            FiberOp::Deferred => Self::assign(cx, dst, &call("em_fiber_deferred", &a[..0])),
            FiberOp::Join => word(cx, op_call("em_fiber_join", dst_repr)),
            FiberOp::Attempt { .. } => {
                let repr = arg_ty(0).map_or(Repr::Void, |t| self.inner_repr(t));
                Self::assign(cx, dst, &op_call("em_fiber_attempt", repr));
            }
            FiberOp::Complete => {
                let repr = args.get(1).map_or(Repr::Void, |o| Self::operand_repr(f, o));
                let w = match args.get(1) {
                    Some(o) if repr != Repr::Void => self.word(f, o),
                    _ => "(em_word){.l = 0}".to_owned(),
                };
                let e = call(
                    "em_deferred_complete",
                    [a[0].as_str(), w.as_str(), repr_code(repr)],
                );
                Self::assign(cx, dst, &e);
            }
            FiberOp::DeferredAwait => word(cx, op_call("em_deferred_await", dst_repr)),
            FiberOp::Poll => {
                let repr = dst_ty.map_or(Repr::Void, |t| self.inner_repr(t));
                Self::assign(cx, dst, &op_call("em_deferred_poll", repr));
            }
            FiberOp::CauseKind { .. } => {
                Self::assign(cx, dst, &call("em_cause_kind", &a[..1]));
            }
            FiberOp::CauseToString => {
                Self::assign(cx, dst, &call("em_cause_to_string", &a[..1]));
            }
            FiberOp::CauseRaise { .. } => cx.w.stmt(&call("em_cause_raise", &a[..1])),
            FiberOp::Semaphore => Self::assign(cx, dst, &call("em_fiber_semaphore", &a[..1])),
            FiberOp::Queue => Self::assign(cx, dst, &call("em_fiber_queue", &a[..1])),
            FiberOp::Acquire => cx.w.stmt(&call("em_semaphore_acquire", &a[..2])),
            FiberOp::TryAcquire => {
                Self::assign(cx, dst, &call("em_semaphore_try_acquire", &a[..2]));
            }
            FiberOp::Release => cx.w.stmt(&call("em_semaphore_release", &a[..2])),
            FiberOp::Available => {
                Self::assign(cx, dst, &call("em_semaphore_available", &a[..1]));
            }
            FiberOp::Offer | FiberOp::TryOffer => {
                // A value without representation is passed as the zero word, as for complete.
                let repr = args.get(1).map_or(Repr::Void, |o| Self::operand_repr(f, o));
                let w = match args.get(1) {
                    Some(o) if repr != Repr::Void => self.word(f, o),
                    _ => "(em_word){.l = 0}".to_owned(),
                };
                let name = if op == FiberOp::Offer {
                    "em_queue_offer"
                } else {
                    "em_queue_try_offer"
                };
                let e = call(name, [a[0].as_str(), w.as_str(), repr_code(repr)]);
                if op == FiberOp::Offer {
                    word(cx, e);
                } else {
                    Self::assign(cx, dst, &e);
                }
            }
            FiberOp::Take => {
                let repr = dst_ty.map_or(Repr::Void, |t| self.inner_repr(t));
                word(cx, op_call("em_queue_take", repr));
            }
            FiberOp::QueuePoll => {
                let repr = dst_ty.map_or(Repr::Void, |t| self.inner_repr(t));
                Self::assign(cx, dst, &op_call("em_queue_poll", repr));
            }
            FiberOp::Size => Self::assign(cx, dst, &call("em_queue_size", &a[..1])),
            FiberOp::Close => cx.w.stmt(&call("em_queue_close", &a[..1])),
        }
    }

    fn nullary(&mut self, desc: usize) -> String {
        let n = match self.nullary.get(&desc) {
            Some(&n) => n,
            None => {
                let n = self.nullary.len();
                self.nullary.insert(desc, n);
                self.statics.decl(
                    "static em_hdr",
                    &format!("em_n{n}"),
                    Some(&format!("EM_STATIC_HDR(&em_d{desc})")),
                );
                n
            }
        };
        format!("((em_ref)&em_n{n})")
    }

    fn static_closure(&mut self, function: FnId) -> String {
        let n = match self.closures.get(&function) {
            Some(&n) => n,
            None => {
                let n = self.closures.len();
                self.closures.insert(function, n);
                let desc = self.desc(DescKey {
                    tag: 0,
                    enum_id: -1,
                    name: display_name(&self.m.functions[function].name).to_owned(),
                    show: None,
                    refs: vec![false],
                    value_size: 0,
                });
                let fn_word = format!("{{.fn = (em_fnptr){}}}", self.fn_names[ix(function)]);
                self.statics.decl(
                    "static struct { em_hdr hdr; em_word fn; }",
                    &format!("em_k{n}"),
                    Some(&init_list([format!("EM_STATIC_HDR(&em_d{desc})"), fn_word])),
                );
                n
            }
        };
        format!("((em_ref)&em_k{n})")
    }

    fn call(&mut self, cx: &mut Cx<'_>, dst: Option<VarId>, callee: &Callee, args: &[Operand]) {
        let f = cx.f;
        let a: Vec<String> = args.iter().map(|o| self.operand(o)).collect();
        match callee {
            Callee::Direct(g) => {
                let e = call(&self.fn_names[ix(*g)], &a);
                Self::assign(cx, dst, &e);
            }
            Callee::Indirect(c) => {
                let Type::Fn(fty) = self.core().ty(f.vars[*c].ty) else {
                    self.internal(format!(
                        "calling a value that is not a function in {}",
                        f.name
                    ));
                    return;
                };
                let ret = self.ct(fty.ret);
                let e = self.closure_call(&var(*c), ret, f, args, &a);
                Self::assign(cx, dst, &e);
            }
            Callee::Op {
                handler,
                effect,
                op,
            } => {
                let def = &self.core().effects[*effect].ops[*op];
                let ret = self.ct(def.ret);
                let h = var(*handler);
                let closure = format!("{h}->slots[{}].p", op + 1);
                cx.w.block_begin();
                if def.suspend {
                    // The implementation takes a resume function value last and returns
                    // nothing; the value to continue with comes from `em_perform_end`.
                    let name = names::string_literal(def.name.as_bytes());
                    let resume = call(
                        "em_resume_new",
                        [h.as_str(), name.as_str(), repr_code_of(ret)],
                    );
                    cx.w.decl("em_ref", "rs", Some(&resume));
                    let mut a = a.clone();
                    a.push("rs".to_owned());
                    let mut args = args.to_vec();
                    args.push(Operand::Const(Const::String(String::new())));
                    let call = self.closure_call(&closure, CTy::Scalar(Repr::Void), f, &args, &a);
                    cx.w.stmt(&call);
                    cx.w.decl("em_word", "pw", Some("em_perform_end(rs)"));
                    if let Some(d) = dst {
                        cx.w.assign(&var(d), &format!("pw.{}", acc(ret.repr())));
                    } else {
                        cx.w.stmt("(void)pw");
                    }
                } else {
                    let e = self.closure_call(&closure, ret, f, args, &a);
                    Self::assign(cx, dst, &e);
                }
                cx.w.end();
            }
        }
    }

    /// Calls the closure record `closure` with itself as the environment, then `args`. The C
    /// function pointer is cast to the types of this call: a parameter or result held by value is
    /// the struct, like a direct call.
    fn closure_call(
        &self,
        closure: &str,
        ret: CTy,
        f: &Function,
        args: &[Operand],
        a: &[String],
    ) -> String {
        let mut params = vec!["em_ref".to_owned()];
        params.extend(
            args.iter()
                .map(|o| self.ty_cty(Self::operand_ty(f, o)).to_owned()),
        );
        let mut call_args = vec![closure.to_owned()];
        call_args.extend(a.iter().cloned());
        format!(
            "(({} (*)({}))em_closure_fn({closure}))({})",
            self.cty(ret),
            params.join(", "),
            call_args.join(", ")
        )
    }
}

/// The runtime's constant for a representation (`EM_REPR_I32`).
fn repr_code_of(t: CTy) -> &'static str {
    repr_code(t.repr())
}

fn repr_code(r: Repr) -> &'static str {
    match r {
        Repr::I32 => "EM_REPR_I32",
        Repr::I64 => "EM_REPR_I64",
        Repr::F64 => "EM_REPR_F64",
        Repr::Ref => "EM_REPR_REF",
        Repr::Void => "EM_REPR_VOID",
    }
}

fn prim(op: PrimOp, ty: PrimTy, a: &[String]) -> String {
    let bin = |sym: &str| format!("({} {sym} {})", a[0], a[1]);
    match ty {
        PrimTy::I32 | PrimTy::I64 => {
            let p = if ty == PrimTy::I32 { "em_i" } else { "em_l" };
            match op {
                PrimOp::Add => format!("{p}add({}, {})", a[0], a[1]),
                PrimOp::Sub => format!("{p}sub({}, {})", a[0], a[1]),
                PrimOp::Mul => format!("{p}mul({}, {})", a[0], a[1]),
                PrimOp::Div => format!("{p}div({}, {})", a[0], a[1]),
                PrimOp::Rem => format!("{p}rem({}, {})", a[0], a[1]),
                PrimOp::Neg => format!("{p}neg({})", a[0]),
                PrimOp::Not => format!("(!{})", a[0]),
                _ => bin(op.symbol()),
            }
        }
        PrimTy::F64 => match op {
            PrimOp::Rem => format!("em_fmod({}, {})", a[0], a[1]),
            PrimOp::Neg => format!("(-{})", a[0]),
            PrimOp::Not => format!("(!{})", a[0]),
            _ => bin(op.symbol()),
        },
        PrimTy::String => match op {
            PrimOp::Eq => format!("em_str_eq({}, {})", a[0], a[1]),
            PrimOp::Ne => format!("(!em_str_eq({}, {}))", a[0], a[1]),
            _ => format!("(em_str_cmp({}, {}) {} 0)", a[0], a[1], op.symbol()),
        },
    }
}
