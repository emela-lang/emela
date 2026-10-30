//! Canonical backend values and definitions, owned independently of Core and Flow IR.
//! Frontend function bodies, name-resolution state, source files, and Heap-IR stages are absent.
//! The upstream adapter maps every input variant explicitly; downstream code uses these IDs only.
use crate::contract::DerivedOperation;
use la_arena::{Arena, Idx};
use std::collections::{HashMap, HashSet};

pub(crate) type FnId = Idx<Function>;
pub(crate) type VarId = Idx<VarData>;
pub(crate) type StmtId = Idx<Stmt>;
pub(crate) type SeqId = Idx<Seq>;
pub(crate) type LabelId = Idx<LabelData>;
pub(crate) type EnumId = Idx<EnumDef>;
pub(crate) type EffectId = Idx<EffectDef>;

#[derive(Debug, Clone, Default)]
pub(crate) struct Definitions {
    pub(crate) types: TypeTable,
    pub(crate) enums: Arena<EnumDef>,
    pub(crate) effects: Arena<EffectDef>,
    pub(crate) expanding_enums: HashSet<EnumId>,
}
impl Definitions {
    pub(crate) fn ty(&self, ty: TypeId) -> &Type {
        &self.types[ty]
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Program {
    pub(crate) core: Definitions,
    pub(crate) functions: Arena<Function>,
    pub(crate) error_shows: Vec<(EnumId, FnId)>,
    pub(crate) const_function_groups: Vec<Vec<FnId>>,
    pub(crate) has_scopes: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) exported: bool,
    pub(crate) params: Vec<VarId>,
    pub(crate) ret: TypeId,
    pub(crate) fails: bool,
    pub(crate) suspends: bool,
    pub(crate) constant: bool,
    pub(crate) external: Option<External>,
    pub(crate) env: Option<Vec<TypeId>>,
    pub(crate) body: Option<SeqId>,
    pub(crate) vars: Arena<VarData>,
    pub(crate) stmts: Arena<Stmt>,
    pub(crate) seqs: Arena<Seq>,
    pub(crate) labels: Arena<LabelData>,
    pub(crate) derivation: Option<DerivedOperation>,
    pub(crate) infallible_cause_fail: bool,
    pub(crate) statement_positions: HashMap<StmtId, usize>,
    pub(crate) sequence_positions: HashMap<SeqId, usize>,
    pub(crate) active_statements: HashSet<StmtId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VarData {
    pub(crate) hint: String,
    pub(crate) repr: Repr,
    pub(crate) ty: TypeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LabelData {
    pub(crate) hint: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Seq {
    pub(crate) stmts: Vec<StmtId>,
    pub(crate) term: Term,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Term {
    Yield(Option<Operand>),
    Return(Option<Operand>),
    Fail(Operand),
    Break {
        label: LabelId,
        value: Option<Operand>,
    },
    Recur(Vec<Operand>),
    Defect(Operand),
    Unwind,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Stmt {
    Let {
        dst: Option<VarId>,
        value: Value,
        on_defect: Option<SeqId>,
        on_suspend: Option<Suspend>,
    },
    If {
        dst: Option<VarId>,
        cond: Operand,
        then: SeqId,
        else_: SeqId,
    },
    Switch {
        dst: Option<VarId>,
        on: SwitchOn,
        cases: Vec<Case>,
        default: Option<SeqId>,
    },
    Loop {
        dst: Option<VarId>,
        vars: Vec<VarId>,
        inits: Vec<Operand>,
        body: SeqId,
    },
    Block {
        dst: Option<VarId>,
        label: LabelId,
        body: SeqId,
    },
    Protect {
        dst: Option<VarId>,
        body: SeqId,
        cleanup: SeqId,
    },
    Dup(VarId),
    Drop(VarId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Suspend {
    pub(crate) state: u32,
    pub(crate) saved: Vec<VarId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchOn {
    Tag(VarId),
    Lit(VarId),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Case {
    pub(crate) key: CaseKey,
    pub(crate) body: SeqId,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CaseKey {
    Variant(usize),
    Lit(Const),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Operand {
    Var(VarId),
    Const(Const),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Const {
    Int(i32),
    Int64(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Unit,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Const(Const),
    Prim {
        op: PrimOp,
        ty: PrimTy,
        args: Vec<Operand>,
    },
    Call {
        callee: Callee,
        args: Vec<Operand>,
        suspends: bool,
        on_fail: Option<OnFail>,
    },
    Builtin {
        op: Builtin,
        args: Vec<Operand>,
    },
    Make {
        ctor: CtorRef,
        args: Vec<Operand>,
    },
    Field {
        base: VarId,
        ctor: CtorRef,
        index: usize,
    },
    Tag(VarId),
    Tuple(Vec<Operand>),
    TupleGet {
        base: VarId,
        index: usize,
    },
    List {
        elems: Vec<Operand>,
        tail: Option<Operand>,
    },
    ListIsEmpty(VarId),
    ListHead(VarId),
    ListTail(VarId),
    FnRef(FnId),
    Closure(Box<Closure>),
    MakeClosure {
        function: FnId,
        captures: Vec<VarId>,
    },
    EnvGet {
        env: VarId,
        index: usize,
    },
    EvLookup {
        ev: VarId,
        effect: EffectId,
    },
    EvExtend {
        base: Option<VarId>,
        bindings: Vec<(EffectId, VarId)>,
    },
    Handler {
        name: String,
        effect: EffectId,
        ops: Vec<VarId>,
    },
    Fiber {
        op: FiberOp,
        args: Vec<Operand>,
        on_fail: Option<OnFail>,
    },
    IsError {
        error: VarId,
        enum_id: EnumId,
    },
    AsError {
        error: VarId,
        enum_id: EnumId,
    },
    Runtime {
        op: RuntimeOp,
        args: Vec<Operand>,
    },
    Coerce(VarId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeOp {
    ShowString,
    DebugPrint,
    Retire,
    Defect,
    DefectAt,
    DefectTake,
    DefectResume,
    Hold,
    Unhold,
    Yield,
    YieldPerform,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Closure {
    pub(crate) params: Vec<VarId>,
    pub(crate) captures: Vec<VarId>,
    pub(crate) body: SeqId,
    pub(crate) suspends: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Callee {
    Direct(FnId),
    Indirect(VarId),
    Op {
        handler: VarId,
        effect: EffectId,
        op: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnFail {
    Break(LabelId),
    Propagate,
    Handle { error: VarId, body: SeqId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimTy {
    I32,
    I64,
    F64,
    String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct External {
    pub(crate) bindings: Vec<ExternalBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalBinding {
    pub(crate) target: String,
    pub(crate) host: HostRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostRef {
    Global(String),
    Import { module: HostModule, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostModule {
    Named(String),
    File(String),
    Bundled(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectDef {
    pub(crate) name: String,
    pub(crate) decl: String,
    pub(crate) args: Vec<TypeId>,
    pub(crate) ops: Vec<OpDef>,
    pub(crate) label_of: Option<EffectId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpDef {
    pub(crate) name: String,
    pub(crate) suspend: bool,
    pub(crate) type_params: usize,
    pub(crate) params: Vec<TypeId>,
    pub(crate) ret: TypeId,
    pub(crate) fails: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct EnumDef {
    pub(crate) name: String,
    pub(crate) params: usize,
    pub(crate) variants: Vec<VariantDef>,
    pub(crate) error: bool,
    pub(crate) external: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct VariantDef {
    pub(crate) name: String,
    pub(crate) fields: VariantFields,
    pub(crate) tys: Vec<TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VariantFields {
    Unit,
    Positional(usize),
    Named(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CtorRef {
    pub(crate) enum_id: EnumId,
    pub(crate) variant: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TypeId(u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Type {
    Int,
    Int64,
    Float,
    Bool,
    String,
    Bytes,
    Unit,
    Never,
    Tuple(Vec<TypeId>),
    List(TypeId),
    Enum(EnumId, Vec<TypeId>),
    Param(usize),
    Fn(FnType),
    Handler(EffectId),
    Evidence,
    Cell(TypeId),
    FiberScope,
    Any,
    Set(SetSummary),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) struct SetSummary {
    pub(crate) fails: bool,
    pub(crate) suspends: bool,
    pub(crate) ev: bool,
    pub(crate) params: Vec<TypeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FnSets {
    pub(crate) errors: TypeId,
    pub(crate) effects: TypeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FnType {
    pub(crate) params: Vec<TypeId>,
    pub(crate) ret: TypeId,
    pub(crate) fails: bool,
    pub(crate) suspends: bool,
    pub(crate) ev: bool,
    pub(crate) sets: Option<FnSets>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FiberOp {
    Scope { fails: bool },
    Fork,
    Deferred,
    Join,
    Await { exit: EnumId },
    Interrupt,
    Attempt { exit: EnumId, suspends: bool },
    Complete,
    DeferredAwait,
    Poll,
    CauseKind { kind: EnumId },
    CauseToString,
    CauseRaise,
}

#[derive(Debug, Clone)]
pub(crate) struct TypeTable {
    pub(crate) types: Vec<Type>,
    pub(crate) has_param: Vec<bool>,
    pub(crate) ids: HashMap<Type, TypeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Builtin {
    StringLength,
    StringByteSize,
    StringAppend,
    StringContains,
    StringStartsWith,
    StringEndsWith,
    StringSplit,
    StringChars,
    StringJoin,
    StringTrim,
    StringCodePoints,
    StringFromCodePoint,
    ShowNested,
    IntToString,
    IntParse,
    IntCheckedAdd,
    IntBitAnd,
    IntBitOr,
    IntBitXor,
    IntBitNot,
    IntShiftLeft,
    IntShiftRight,
    IntShiftRightUnsigned,
    Int64FromInt,
    Int64ToString,
    Int64ToInt,
    Int64BitAnd,
    Int64BitOr,
    Int64BitXor,
    Int64BitNot,
    Int64ShiftLeft,
    Int64ShiftRight,
    Int64ShiftRightUnsigned,
    FloatToString,
    FloatFromInt,
    FloatParse,
    FloatFloor,
    FloatCeil,
    FloatRound,
    FloatTruncate,
    BytesFromList,
    BytesToList,
    BytesSize,
    BytesGet,
    BytesSlice,
    BytesConcat,
    BytesEncodeUtf8,
    BytesDecodeValidUtf8,
    Panic,
    AssertFail,
    Todo,
    Dbg,
    Inspect,
    CellNew,
    CellGet,
    CellSet,
    ShowString,
    DebugPrint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Repr {
    I32,
    I64,
    F64,
    Ref,
    Void,
}

impl TypeId {
    pub const INT: TypeId = TypeId(0);
    pub const INT64: TypeId = TypeId(1);
    pub const FLOAT: TypeId = TypeId(2);
    pub const BOOL: TypeId = TypeId(3);
    pub const STRING: TypeId = TypeId(4);
    pub const UNIT: TypeId = TypeId(5);
    pub const NEVER: TypeId = TypeId(6);
    pub const EVIDENCE: TypeId = TypeId(7);
    pub const FIBER_SCOPE: TypeId = TypeId(8);
    pub const ANY: TypeId = TypeId(9);
    pub const BYTES: TypeId = TypeId(10);
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Debug for TypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TypeId({})", self.0)
    }
}

impl VariantFields {
    pub fn len(&self) -> usize {
        match self {
            VariantFields::Unit => 0,
            VariantFields::Positional(n) => *n,
            VariantFields::Named(names) => names.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl FnType {}

impl External {
    pub fn binding(&self, target: &str) -> Option<&ExternalBinding> {
        self.bindings.iter().find(|b| b.target == target)
    }
}

impl std::ops::Index<TypeId> for TypeTable {
    type Output = Type;
    fn index(&self, id: TypeId) -> &Type {
        &self.types[id.index()]
    }
}

impl Stmt {
    pub fn dst(&self) -> Option<VarId> {
        match self {
            Stmt::Let { dst, .. }
            | Stmt::If { dst, .. }
            | Stmt::Switch { dst, .. }
            | Stmt::Loop { dst, .. }
            | Stmt::Block { dst, .. }
            | Stmt::Protect { dst, .. } => *dst,
            Stmt::Dup(_) | Stmt::Drop(_) => None,
        }
    }
}

impl Const {}

impl PrimOp {}

impl PrimTy {}

impl RuntimeOp {}

impl FiberOp {}

impl Repr {
    pub fn name(self) -> &'static str {
        match self {
            Repr::I32 => "i32",
            Repr::I64 => "i64",
            Repr::F64 => "f64",
            Repr::Ref => "ref",
            Repr::Void => "void",
        }
    }
}

impl std::fmt::Display for Repr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Value {
    pub fn for_each_var(&self, mut f: impl FnMut(VarId)) {
        let ops = |os: &[Operand], f: &mut dyn FnMut(VarId)| {
            for o in os {
                if let Operand::Var(v) = o {
                    f(*v);
                }
            }
        };
        match self {
            Value::Const(_) | Value::FnRef(_) => {}
            Value::Prim { args, .. }
            | Value::Builtin { args, .. }
            | Value::Make { args, .. }
            | Value::Tuple(args)
            | Value::Fiber { args, .. }
            | Value::Runtime { args, .. } => ops(args, &mut f),
            Value::Call { callee, args, .. } => {
                match callee {
                    Callee::Direct(_) => {}
                    Callee::Indirect(v) | Callee::Op { handler: v, .. } => f(*v),
                }
                ops(args, &mut f);
            }
            Value::Field { base: v, .. }
            | Value::Tag(v)
            | Value::TupleGet { base: v, .. }
            | Value::ListIsEmpty(v)
            | Value::ListHead(v)
            | Value::ListTail(v)
            | Value::EvLookup { ev: v, .. }
            | Value::IsError { error: v, .. }
            | Value::AsError { error: v, .. }
            | Value::Coerce(v) => f(*v),
            Value::List { elems, tail } => {
                ops(elems, &mut f);
                ops(tail.as_slice(), &mut f);
            }
            Value::Closure(c) => c.captures.iter().for_each(|&v| f(v)),
            Value::MakeClosure { captures, .. } => captures.iter().for_each(|&v| f(v)),
            Value::EnvGet { env, .. } => f(*env),
            Value::EvExtend { base, bindings } => {
                base.iter().for_each(|&v| f(v));
                bindings.iter().for_each(|&(_, v)| f(v));
            }
            Value::Handler { ops: hs, .. } => hs.iter().for_each(|&v| f(v)),
        }
    }
}

impl From<VarId> for Operand {
    fn from(value: VarId) -> Self {
        Self::Var(value)
    }
}
impl From<Const> for Operand {
    fn from(value: Const) -> Self {
        Self::Const(value)
    }
}

impl TypeId {
    pub(crate) fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("backend type table exceeds u32"))
    }
}
pub(crate) const EXIT_SUCCESS: usize = 0;
pub(crate) const EXIT_FAILURE: usize = 1;
pub(crate) const CAUSE_FAIL: usize = 0;
pub(crate) const CAUSE_INTERRUPT: usize = 2;

impl Function {
    pub fn new(name: &str) -> Self {
        Function {
            derivation: None,
            infallible_cause_fail: false,
            statement_positions: HashMap::new(),
            sequence_positions: HashMap::new(),
            active_statements: HashSet::new(),
            name: name.to_owned(),
            exported: false,
            params: Vec::new(),
            ret: TypeId::UNIT,
            fails: false,
            suspends: false,
            constant: false,
            external: None,
            env: None,
            body: None,
            vars: Arena::new(),
            stmts: Arena::new(),
            seqs: Arena::new(),
            labels: Arena::new(),
        }
    }
}
