//! WebAssembly backend (Tier 1).
//!
//! Lowers the Emela IR to a WASI module that runs on WAMR (`iwasm`) or any
//! other WASI preview1 runtime. The module is authored as WAT text and then
//! assembled to a binary with the pure-Rust [`wat`] crate, so the compiler
//! needs no external wasm tools.
//!
//! ## Representation
//! - `Int` / `Bool` / `Unit` -> `i32` (`Unit` is the constant `0`)
//! - `Float` -> `f64`
//! - `String` / `Array` / function values -> `i32` pointers into linear memory
//!
//! ## First-class functions
//! Every Emela function (top level and `fn` lambda) is closure-converted to a
//! wasm function taking an environment pointer as its first parameter, and is
//! placed in a function table. A function *value* is a pointer to a closure
//! `[table_index: i32, capture0, capture1, ...]`. A direct call to a known
//! top-level function uses `call`; calling any other function value loads the
//! table index from the closure and uses `call_indirect`.
//!
//! ## Execution
//! `_start` calls `main`. If `main` returns `Int`, that value is the process
//! exit code; otherwise the result is dropped and the exit code is `0`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use emela_codegen::{
    Artifact, ArtifactKind, Backend, BackendError, BackendOptions, BinaryOp, EmitMode,
    FunctionType, IrArm, IrCapture, IrExpr, IrFunction, IrParam, IrPattern, IrProgram, Result,
    Tier, Type, contains_tail_self_call, insert_rc_ops, is_heap, used_intrinsics,
    used_platform_fns, walk,
};

/// The WASI/WAMR WebAssembly backend.
pub struct WasmBackend;

impl Backend for WasmBackend {
    fn name(&self) -> &str {
        "wasm-wasi"
    }

    fn tier(&self) -> Tier {
        Tier::Tier1
    }

    fn compile(&self, ir: &IrProgram, options: &BackendOptions) -> Result<Artifact> {
        // ARC (spec 0048): insert retain/release on a private copy, after
        // lowering already ran the tail-call rewrite (0045). Other backends'
        // IR stream stays untouched.
        let mut ir = ir.clone();
        insert_rc_ops(&mut ir);
        let wat = emit_module(&ir, &options.platform_registry)?;
        match options.mode {
            EmitMode::Text => Ok(Artifact::text(ArtifactKind::WasmText, wat)),
            EmitMode::Default => {
                // Assemble the WAT to a binary, then type-validate the module so
                // a malformed module fails here rather than at run time.
                let bytes = wat::parse_str(&wat).map_err(|err| {
                    BackendError::with(
                        "internal error: generated WAT failed to assemble".to_string(),
                        vec![err.to_string()],
                    )
                })?;
                wasmparser::validate(&bytes).map_err(|err| {
                    BackendError::with(
                        "internal error: generated wasm failed to validate".to_string(),
                        vec![err.to_string()],
                    )
                })?;
                Ok(Artifact {
                    kind: ArtifactKind::WasmBinary,
                    bytes,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Value representation
// ---------------------------------------------------------------------------

/// The numeric WebAssembly type a value is represented with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WasmTy {
    I32,
    F64,
}

impl WasmTy {
    fn of(ty: &Type) -> WasmTy {
        match ty {
            Type::Float => WasmTy::F64,
            _ => WasmTy::I32,
        }
    }

    fn keyword(self) -> &'static str {
        match self {
            WasmTy::I32 => "i32",
            WasmTy::F64 => "f64",
        }
    }

    fn size(self) -> u32 {
        match self {
            WasmTy::I32 => 4,
            WasmTy::F64 => 8,
        }
    }

    fn load(self) -> &'static str {
        match self {
            WasmTy::I32 => "i32.load",
            WasmTy::F64 => "f64.load",
        }
    }

    fn store(self) -> &'static str {
        match self {
            WasmTy::I32 => "i32.store",
            WasmTy::F64 => "f64.store",
        }
    }
}

/// A wasm function signature, with the leading environment pointer included.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WasmSig {
    params: Vec<WasmTy>,
    result: WasmTy,
}

impl WasmSig {
    fn from_parts(params: impl Iterator<Item = WasmTy>, ret: WasmTy) -> WasmSig {
        let mut all = vec![WasmTy::I32]; // environment pointer
        all.extend(params);
        WasmSig {
            params: all,
            result: ret,
        }
    }

    fn of_fn(params: &[IrParam], ret: &Type, throws: &Option<Type>) -> WasmSig {
        let result = result_wasm_ty(ret, throws.is_some());
        WasmSig::from_parts(params.iter().map(|p| WasmTy::of(&p.ty)), result)
    }

    fn of_type(ty: &FunctionType) -> WasmSig {
        let result = result_wasm_ty(&ty.ret, ty.throws.is_some());
        WasmSig::from_parts(ty.params.iter().map(WasmTy::of), result)
    }
}

/// The wasm result type of a function: a throwing function returns a Result
/// pointer (`i32`, `[ok:i32][value]`); otherwise its plain value type.
fn result_wasm_ty(ret: &Type, throwing: bool) -> WasmTy {
    if throwing {
        WasmTy::I32
    } else {
        WasmTy::of(ret)
    }
}

/// Whether a call's callee is a throwing function, so its result is a Result
/// pointer that the caller must unwrap (spec 0011).
fn is_throwing(callee: &IrExpr) -> bool {
    matches!(callee.ty(), Type::Function(ft) if ft.throws.is_some())
}

const STRING_BASE: u32 = 16;
const MEMORY_PAGES: u32 = 16;
/// Upper bound the linear memory may grow to (spec 0048 A3): 256 pages = 16 MiB.
/// Reaching it makes the next allocation trap (OOM panic).
const MEMORY_MAX_PAGES: u32 = 256;

fn align(value: u32, to: u32) -> u32 {
    (value + to - 1) & !(to - 1)
}

// ---------------------------------------------------------------------------
// Static string data
// ---------------------------------------------------------------------------

/// Interned string literals laid out in linear memory as `[len: i32][utf8]`.
struct StringTable {
    offsets: HashMap<String, u32>,
    segments: Vec<(u32, Vec<u8>)>,
    heap_start: u32,
}

fn collect_strings(ir: &IrProgram) -> StringTable {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for function in &ir.functions {
        walk(&function.body, &mut |expr| {
            if let IrExpr::String(value) = expr
                && seen.insert(value.clone())
            {
                order.push(value.clone());
            }
        });
    }
    let mut offsets = HashMap::new();
    let mut segments = Vec::new();
    let mut cursor = STRING_BASE;
    for literal in order {
        let offset = align(cursor, 4);
        let mut bytes = (literal.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(literal.as_bytes());
        cursor = offset + bytes.len() as u32;
        offsets.insert(literal, offset);
        segments.push((offset, bytes));
    }
    StringTable {
        offsets,
        segments,
        heap_start: align(cursor, 8),
    }
}

/// Renders raw bytes as a WAT data string using only `\HH` hex escapes.
fn wat_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4);
    for byte in bytes {
        let _ = write!(out, "\\{byte:02x}");
    }
    out
}

// ---------------------------------------------------------------------------
// Function table (closure conversion)
// ---------------------------------------------------------------------------

/// Maps every callable to its index in the wasm function table.
struct FnTable<'a> {
    /// Top-level function name -> table index (0..n_top).
    toplevel: HashMap<&'a str, u32>,
    /// `fn` lambda nodes, in collection order; table index = `n_top + i`.
    lambdas: Vec<&'a IrExpr>,
    /// Identity map from a lambda node to its table index.
    lambda_index: HashMap<*const IrExpr, u32>,
    n_top: u32,
}

impl<'a> FnTable<'a> {
    fn build(ir: &'a IrProgram) -> FnTable<'a> {
        let n_top = ir.functions.len() as u32;
        let mut toplevel = HashMap::new();
        for (index, function) in ir.functions.iter().enumerate() {
            toplevel.insert(function.name.as_str(), index as u32);
        }
        let mut lambdas = Vec::new();
        let mut lambda_index = HashMap::new();
        for function in &ir.functions {
            walk(&function.body, &mut |expr| {
                if let IrExpr::Fn { .. } = expr {
                    let index = n_top + lambdas.len() as u32;
                    lambdas.push(expr);
                    lambda_index.insert(expr as *const IrExpr, index);
                }
            });
        }
        FnTable {
            toplevel,
            lambdas,
            lambda_index,
            n_top,
        }
    }

    fn is_direct<'b>(&self, callee: &'b IrExpr) -> Option<&'b str> {
        match callee {
            IrExpr::FunctionRef { name, .. } if self.toplevel.contains_key(name.as_str()) => {
                Some(name)
            }
            _ => None,
        }
    }
}

/// Whether a `host.*` platform function (spec 0026) is registered and thus
/// provided by this backend. The host runtime supplies the actual
/// implementation at instantiation time.
fn host_platform_provided(
    canonical: &str,
    platform_registry: &[emela_codegen::PlatformFn],
) -> bool {
    canonical.starts_with("host.")
        && emela_codegen::platform_lookup_in(platform_registry, canonical).is_some()
}

/// The WASM import a `host.*` platform function lowers to, or `None` if the
/// function is not a host platform function. The import module name follows
/// the `host_<name>` convention (spec 0026 Compilation Notes).
fn host_platform_import(
    canonical: &str,
    platform_registry: &[emela_codegen::PlatformFn],
) -> Option<String> {
    if !canonical.starts_with("host.") {
        return None;
    }
    let entry = emela_codegen::platform_lookup_in(platform_registry, canonical)?;
    let (module_part, func) = split_host_canonical(canonical)?;
    let host_module = format!("host_{module_part}");
    let wasm_import_name = platform_wasm_name(canonical).replace('$', "$host_");
    let param_str = wasm_params(&entry.params);
    let result_str = wasm_result(&entry.ret);
    Some(format!(
        "  (import \"{host_module}\" \"{func}\" (func {wasm_import_name} {param_str}{result_str}))\n"
    ))
}

/// The runtime glue for a `host.*` platform function: a wrapper that
/// forwards its parameters to the host import.
fn host_platform_glue(
    canonical: &str,
    platform_registry: &[emela_codegen::PlatformFn],
) -> Option<String> {
    if !canonical.starts_with("host.") {
        return None;
    }
    let entry = emela_codegen::platform_lookup_in(platform_registry, canonical)?;
    let wasm_name = platform_wasm_name(canonical);
    let host_name = platform_wasm_name(canonical).replace('$', "$host_");
    let n_params = entry.params.len();
    let result_type = if entry.ret == emela_codegen::Type::Unit {
        " (result i32)".to_string()
    } else {
        format!(" (result {})", wasm_type_for(&entry.ret))
    };
    let mut glue = format!(
        "  (func {wasm_name} {}{result_type}\n",
        wasm_param_decls(n_params),
    );
    // Forward the parameters.
    for i in 0..n_params {
        glue.push_str(&format!("    local.get {i}\n"));
    }
    // Call the host import.
    glue.push_str(&format!("    call {host_name}\n"));
    // Push a dummy i32 for Unit-returning functions (all WASM glue
    // functions produce an i32 to satisfy the backend's calling convention).
    if entry.ret == emela_codegen::Type::Unit {
        glue.push_str("    i32.const 0\n");
    }
    glue.push_str("  )\n");
    Some(glue)
}

/// Splits a `host.<name>.<func>` canonical into `("name", "func")`.
fn split_host_canonical(canonical: &str) -> Option<(&str, &str)> {
    let rest = canonical.strip_prefix("host.")?;
    let dot = rest.find('.')?;
    Some((&rest[..dot], &rest[dot + 1..]))
}

/// WAT parameter declarations for a host platform function.
fn wasm_param_decls(count: usize) -> String {
    (0..count)
        .map(|i| format!("(param $p{i} i32)"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// WAT parameter types (without names) for an import signature.
fn wasm_params(params: &[emela_codegen::Type]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let params_str = params.iter().map(|_| "i32").collect::<Vec<_>>().join(" ");
    format!("(param {params_str}) ")
}

/// WAT result type for an import signature.
fn wasm_result(ret: &emela_codegen::Type) -> String {
    if *ret == emela_codegen::Type::Unit {
        String::new()
    } else {
        format!("(result {})", wasm_type_for(ret))
    }
}

/// Maps an Emela type to its WAT representation (simplified: all host
/// functions use i32 for scalar values and pointers).
fn wasm_type_for(ty: &emela_codegen::Type) -> &'static str {
    match ty {
        emela_codegen::Type::Float => "f64",
        emela_codegen::Type::Unit => "",
        _ => "i32",
    }
}

/// Interns the distinct wasm signatures used as `call_indirect` types.
#[derive(Default)]
struct SigTable {
    list: Vec<WasmSig>,
    index: HashMap<WasmSig, usize>,
}

impl SigTable {
    fn add(&mut self, sig: WasmSig) {
        if !self.index.contains_key(&sig) {
            self.index.insert(sig.clone(), self.list.len());
            self.list.push(sig);
        }
    }

    fn index_of(&self, sig: &WasmSig) -> Option<usize> {
        self.index.get(sig).copied()
    }
}

fn build_sigs(ir: &IrProgram, table: &FnTable) -> SigTable {
    let mut sigs = SigTable::default();
    // Every table entry can be the target of an indirect call.
    for function in &ir.functions {
        sigs.add(WasmSig::of_fn(
            &function.params,
            &function.ret,
            &function.throws,
        ));
    }
    for lambda in &table.lambdas {
        if let IrExpr::Fn {
            params,
            ret,
            throws,
            ..
        } = lambda
        {
            sigs.add(WasmSig::of_fn(params, ret, throws));
        }
    }
    // And every indirect call site needs its signature declared.
    for function in &ir.functions {
        walk(&function.body, &mut |expr| {
            if let IrExpr::Call { callee, .. } = expr
                && table.is_direct(callee).is_none()
                && let Type::Function(ft) = callee.ty()
            {
                sigs.add(WasmSig::of_type(&ft));
            }
        });
    }
    sigs
}

fn capture_layout(captures: &[IrCapture]) -> Vec<(String, u32, WasmTy)> {
    let mut out = Vec::new();
    let mut offset = 4; // after the table-index header
    for capture in captures {
        let ty = WasmTy::of(&capture.ty);
        out.push((capture.name.clone(), offset, ty));
        offset += ty.size();
    }
    out
}

fn closure_size(captures: &[IrCapture]) -> u32 {
    4 + captures
        .iter()
        .map(|c| WasmTy::of(&c.ty).size())
        .sum::<u32>()
}

// ---------------------------------------------------------------------------
// Drop glue (spec 0048)
//
// `$rc_release` dispatches a zero-count block to the drop function whose
// funcref-table index sits in the block's header. One function is generated
// per distinct heap *shape*; every allocation site knows its shape statically
// and stamps the index at `$alloc` time. A drop function releases the shape's
// child pointers (each child dispatches through its own header — no
// transitive type analysis needed) and frees the block with its computed
// total size (payload + 8-byte header, 8-aligned).
// ---------------------------------------------------------------------------

/// A heap shape with its own drop function.
#[derive(Clone, PartialEq, Eq, Hash)]
enum DropKey {
    /// Never dropped meaningfully: Result boxes (freed manually at unwrap)
    /// and host-built platform values, whose shape only the host knows — a
    /// released one stays allocated. Follow-up: a host shape protocol.
    Leak,
    /// `[len][utf8]`: free only.
    String,
    /// `[len][elem × stride]`: release elements if they are pointers, free.
    Array { stride: u32, heap_elems: bool },
    /// A tagged enum value, keyed by its mangled type name; the per-tag
    /// payload shapes live in [`DropTable::enum_tags`].
    Enum(String),
    /// A record / fixed block of 8-byte slots; `mask` marks pointer slots.
    Record { mask: Vec<bool> },
    /// A closure environment: `[table_index][captures...]` with pointer
    /// captures at the masked offsets.
    Closure { slots: Vec<(u32, bool)>, size: u32 },
    /// A fixed-size block with no children (a payloadless enum value built by
    /// the backend, a captureless `FunctionRef` closure). `total` includes
    /// the header.
    Plain { total: u32 },
}

struct DropTable {
    /// Table index of the first drop function (after top-level fns + lambdas).
    base: u32,
    keys: Vec<DropKey>,
    names: Vec<String>,
    index: HashMap<DropKey, u32>,
    /// Mangled enum type name -> tag -> which payload slots are pointers.
    enum_tags: HashMap<String, BTreeMap<u32, Vec<bool>>>,
}

/// A stable symbol fragment for a type: alphanumerics kept, the rest `_`.
fn mangle(ty: &Type) -> String {
    let mut out = String::new();
    let mut render = String::new();
    let _ = write!(render, "{ty:?}");
    for ch in render.chars() {
        out.push(if ch.is_ascii_alphanumeric() { ch } else { '_' });
    }
    out
}

impl DropTable {
    fn build(ir: &IrProgram, base: u32) -> DropTable {
        let mut table = DropTable {
            base,
            keys: Vec::new(),
            names: Vec::new(),
            index: HashMap::new(),
            enum_tags: HashMap::new(),
        };
        // Always present: the placeholder for untracked blocks, and strings
        // (concat/slice results exist in almost every program; both are tiny).
        table.add(DropKey::Leak);
        table.add(DropKey::String);
        for function in &ir.functions {
            walk(&function.body, &mut |expr| match expr {
                IrExpr::Array { elem_ty, .. } => {
                    table.add(Self::array_key(elem_ty));
                }
                IrExpr::Intrinsic {
                    name,
                    ret: Type::Array(elem_ty),
                    ..
                } if name == "array_push" => {
                    table.add(Self::array_key(elem_ty));
                }
                IrExpr::EnumValue {
                    ty, tag, payload, ..
                } => {
                    table.register_enum_tag(
                        ty,
                        *tag,
                        payload.iter().map(|field| is_heap(&field.ty())).collect(),
                    );
                }
                IrExpr::RecordValue { fields, .. } => {
                    table.add(DropKey::Record {
                        mask: fields.iter().map(|field| is_heap(&field.ty())).collect(),
                    });
                }
                IrExpr::Fn { captures, .. } => {
                    table.add(Self::closure_key(captures));
                }
                IrExpr::FunctionRef { .. } => {
                    // As a value it becomes a captureless closure; in callee
                    // position no closure is built and the entry goes unused.
                    table.add(DropKey::Plain { total: 16 });
                }
                _ => {}
            });
        }
        table
    }

    fn array_key(elem_ty: &Type) -> DropKey {
        DropKey::Array {
            stride: WasmTy::of(elem_ty).size(),
            heap_elems: is_heap(elem_ty),
        }
    }

    fn closure_key(captures: &[IrCapture]) -> DropKey {
        let slots = capture_layout(captures)
            .into_iter()
            .zip(captures)
            .map(|((_, offset, _), capture)| (offset, is_heap(&capture.ty)))
            .collect();
        DropKey::Closure {
            slots,
            size: closure_size(captures),
        }
    }

    fn register_enum_tag(&mut self, ty: &Type, tag: u32, mask: Vec<bool>) {
        let name = mangle(ty);
        self.add(DropKey::Enum(name.clone()));
        self.enum_tags.entry(name).or_default().insert(tag, mask);
    }

    fn add(&mut self, key: DropKey) -> u32 {
        if let Some(&index) = self.index.get(&key) {
            return index;
        }
        let index = self.base + self.keys.len() as u32;
        let name = match &key {
            DropKey::Leak => "$drop_leak".to_string(),
            DropKey::String => "$drop_string".to_string(),
            DropKey::Array { stride, heap_elems } => {
                format!(
                    "$drop_array_{stride}{}",
                    if *heap_elems { "p" } else { "v" }
                )
            }
            DropKey::Enum(name) => format!("$drop_enum_{name}"),
            DropKey::Record { mask } => format!(
                "$drop_record_{}",
                mask.iter()
                    .map(|&m| if m { '1' } else { '0' })
                    .collect::<String>()
            ),
            DropKey::Closure { .. } => format!("$drop_closure_{}", self.keys.len()),
            DropKey::Plain { total } => format!("$drop_plain_{total}"),
        };
        self.index.insert(key.clone(), index);
        self.keys.push(key);
        self.names.push(name);
        index
    }

    /// The header index for an allocation of this shape. Every shape was
    /// registered during [`DropTable::build`]'s walk of the same IR, so a
    /// miss is a compiler bug.
    fn of(&self, key: &DropKey) -> u32 {
        *self
            .index
            .get(key)
            .expect("drop shape registered during collection")
    }
}

/// `align8(payload + 8)`: the total block size `$free` takes.
fn block_total(payload: u32) -> u32 {
    (payload + 8 + 7) & !7
}

fn emit_drop_glue(drops: &DropTable) -> String {
    let mut out = String::new();
    for (key, name) in drops.keys.iter().zip(&drops.names) {
        match key {
            DropKey::Leak => {
                let _ = writeln!(out, "  (func {name} (param $p i32))");
            }
            DropKey::String => {
                // total = align8(4 + len + 8) = (len + 19) & -8
                let _ = writeln!(
                    out,
                    "  (func {name} (param $p i32)\n    local.get $p\n    local.get $p i32.load i32.const 19 i32.add i32.const -8 i32.and\n    call $free)"
                );
            }
            DropKey::Array { stride, heap_elems } => {
                let _ = writeln!(out, "  (func {name} (param $p i32)");
                if *heap_elems {
                    let _ = writeln!(
                        out,
                        "    (local $i i32) (local $n i32)\n    local.get $p i32.load local.set $n\n    i32.const 0 local.set $i\n    block $done\n      loop $each\n        local.get $i local.get $n i32.ge_s br_if $done\n        local.get $p i32.const 4 i32.add local.get $i i32.const {stride} i32.mul i32.add i32.load\n        call $rc_release\n        local.get $i i32.const 1 i32.add local.set $i\n        br $each\n      end\n    end"
                    );
                }
                // total = align8(4 + n*stride + 8) = (n*stride + 19) & -8
                let _ = writeln!(
                    out,
                    "    local.get $p\n    local.get $p i32.load i32.const {stride} i32.mul i32.const 19 i32.add i32.const -8 i32.and\n    call $free)"
                );
            }
            DropKey::Enum(type_name) => {
                let _ = writeln!(out, "  (func {name} (param $p i32) (local $t i32)");
                let _ = writeln!(out, "    local.get $p i32.load local.set $t");
                for (tag, mask) in &drops.enum_tags[type_name] {
                    let _ = writeln!(out, "    local.get $t i32.const {tag} i32.eq\n    if");
                    for (slot, is_ptr) in mask.iter().enumerate() {
                        if *is_ptr {
                            let offset = 8 + slot as u32 * 8;
                            let _ = writeln!(
                                out,
                                "      local.get $p i32.const {offset} i32.add i32.load call $rc_release"
                            );
                        }
                    }
                    let total = block_total(8 + mask.len() as u32 * 8);
                    let _ = writeln!(
                        out,
                        "      local.get $p i32.const {total} call $free\n      return\n    end"
                    );
                }
                // A tag never constructed by the program cannot reach zero.
                let _ = writeln!(out, "    unreachable)");
            }
            DropKey::Record { mask } => {
                let _ = writeln!(out, "  (func {name} (param $p i32)");
                for (slot, is_ptr) in mask.iter().enumerate() {
                    if *is_ptr {
                        let offset = slot as u32 * 8;
                        let _ = writeln!(
                            out,
                            "    local.get $p i32.const {offset} i32.add i32.load call $rc_release"
                        );
                    }
                }
                let total = block_total((mask.len() as u32 * 8).max(8));
                let _ = writeln!(out, "    local.get $p i32.const {total} call $free)");
            }
            DropKey::Closure { slots, size } => {
                let _ = writeln!(out, "  (func {name} (param $p i32)");
                for (offset, is_ptr) in slots {
                    if *is_ptr {
                        let _ = writeln!(
                            out,
                            "    local.get $p i32.const {offset} i32.add i32.load call $rc_release"
                        );
                    }
                }
                let total = block_total(*size);
                let _ = writeln!(out, "    local.get $p i32.const {total} call $free)");
            }
            DropKey::Plain { total } => {
                let _ = writeln!(
                    out,
                    "  (func {name} (param $p i32)\n    local.get $p i32.const {total} call $free)"
                );
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Module assembly
// ---------------------------------------------------------------------------

fn emit_module(ir: &IrProgram, platform_registry: &[emela_codegen::PlatformFn]) -> Result<String> {
    let main = ir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .ok_or_else(|| BackendError::new("wasm backend requires a `main` function"))?;

    let used_platform = used_platform_fns(ir);
    for name in &used_platform {
        if platform_glue(name).is_none() && !host_platform_provided(name, platform_registry) {
            return Err(BackendError::new(format!(
                "backend `wasm-wasi` does not provide platform function `{name}`"
            )));
        }
    }
    // Intrinsic coverage (spec 0021): reject a program that uses an intrinsic
    // this backend does not inline.
    for name in used_intrinsics(ir) {
        if !wasm_provides_intrinsic(&name) {
            return Err(BackendError::new(format!(
                "backend `wasm-wasi` does not provide intrinsic `{name}`"
            )));
        }
    }

    let strings = collect_strings(ir);
    let table = FnTable::build(ir);
    let sigs = build_sigs(ir, &table);
    let drops = DropTable::build(ir, table.n_top + table.lambdas.len() as u32);
    let ctx = Ctx {
        table: &table,
        sigs: &sigs,
        strings: &strings,
        drops: &drops,
    };

    let mut functions = String::new();
    for function in &ir.functions {
        functions.push_str(&emit_function(function, &ctx)?);
    }
    for lambda in &table.lambdas {
        functions.push_str(&emit_lambda(lambda, &ctx)?);
    }

    let mut module = String::new();
    module.push_str(";; Generated by the Emela wasm backend.\n");
    module.push_str("(module\n");
    module.push_str(
        "  (import \"wasi_snapshot_preview1\" \"proc_exit\" (func $proc_exit (param i32)))\n",
    );
    if !used_platform.is_empty() {
        module.push_str(
            "  (import \"wasi_snapshot_preview1\" \"fd_write\" (func $wasi_fd_write (param i32 i32 i32 i32) (result i32)))\n",
        );
    }
    for name in &used_platform {
        if let Some(import) = platform_import(name) {
            module.push_str(import);
        } else if let Some(import) = host_platform_import(name, platform_registry) {
            module.push_str(&import);
        }
    }
    let _ = writeln!(
        module,
        "  (memory (export \"memory\") {MEMORY_PAGES} {MEMORY_MAX_PAGES})\n  (global $heap (export \"heap_top\") (mut i32) (i32.const {}))\n  (global $free_list (mut i32) (i32.const 0))\n  (global $live (export \"live_bytes\") (mut i32) (i32.const 0))",
        strings.heap_start
    );
    for (offset, bytes) in &strings.segments {
        let _ = writeln!(
            module,
            "  (data (i32.const {offset}) \"{}\")",
            wat_bytes(bytes)
        );
    }
    module.push_str(&emit_types(&sigs));
    module.push_str(&emit_table(&table, &drops));
    module.push_str(&runtime_wat(strings.heap_start, drops.of(&DropKey::Leak)));
    module.push_str(STRING_CMP);
    module.push_str(&string_index_wat(drops.of(&DropKey::String)));
    module.push_str(&bytes_helpers_wat(drops.of(&DropKey::String)));
    for name in &used_platform {
        if let Some(glue) = platform_glue(name) {
            module.push_str(glue);
        } else if let Some(glue) = host_platform_glue(name, platform_registry) {
            module.push_str(&glue);
        }
    }
    module.push_str(&functions);
    module.push_str(&emit_drop_glue(&drops));
    module.push_str(&emit_start(main));
    module.push_str("  (export \"main\" (func $f_main))\n");
    module.push_str("  (export \"_start\" (func $_start))\n");
    if used_platform
        .iter()
        .any(|name| name.starts_with("http.") || name.starts_with("socket."))
    {
        // The HTTP and Socket host functions (specs 0043/0044/0050) allocate
        // their structured results (records / errors / the Result cell) into
        // guest memory, so an allocator is exported alongside `memory`.
        // `$alloc_host` keeps the historical one-parameter shape the host binds
        // against.
        module.push_str("  (export \"alloc\" (func $alloc_host))\n");
    }
    // Capability manifest (spec 0025): embed the program's requirements as a
    // custom section so hosts can audit before instantiation.
    let manifest = emela_codegen::compute_manifest(ir, platform_registry);
    let manifest_json = emela_codegen::serialize_manifest(&manifest);
    let manifest_bytes = wat_bytes(manifest_json.as_bytes());
    let _ = writeln!(
        module,
        "  (@custom \"emela:capabilities\" (after last) \"{manifest_bytes}\")"
    );
    module.push_str(")\n");
    Ok(module)
}

/// The host import a platform function's glue calls through, if it is backed
/// by a dedicated host function rather than WASI. The `emela_http` module is
/// supplied by the `emela run` wasmi host (and any embedder providing the
/// `Http` capability).
fn platform_import(canonical: &str) -> Option<&'static str> {
    match canonical {
        "http.request" => Some(
            "  (import \"emela_http\" \"request\" (func $host_http_request (param i32) (result i32)))\n",
        ),
        // The Socket capability (spec 0050): raw TCP supplied by the `emela run`
        // wasmi host through the `emela_socket` module (the component backend
        // lowers these to `wasi:sockets` instead).
        "socket.raw_listen" => Some(
            "  (import \"emela_socket\" \"raw_listen\" (func $host_socket_raw_listen (param i32) (result i32)))\n",
        ),
        "socket.raw_accept" => Some(
            "  (import \"emela_socket\" \"raw_accept\" (func $host_socket_raw_accept (param i32) (result i32)))\n",
        ),
        "socket.raw_read" => Some(
            "  (import \"emela_socket\" \"raw_read\" (func $host_socket_raw_read (param i32 i32) (result i32)))\n",
        ),
        "socket.raw_write" => Some(
            "  (import \"emela_socket\" \"raw_write\" (func $host_socket_raw_write (param i32 i32) (result i32)))\n",
        ),
        "socket.raw_close" => Some(
            "  (import \"emela_socket\" \"raw_close\" (func $host_socket_raw_close (param i32) (result i32)))\n",
        ),
        // The Socket capability (spec 0050): raw TCP supplied by the `emela run`
        // wasmi host through the `emela_socket` module (the component backend
        // lowers these to `wasi:sockets` instead).
        "socket.raw_listen" => Some(
            "  (import \"emela_socket\" \"raw_listen\" (func $host_socket_raw_listen (param i32) (result i32)))\n",
        ),
        "socket.raw_accept" => Some(
            "  (import \"emela_socket\" \"raw_accept\" (func $host_socket_raw_accept (param i32) (result i32)))\n",
        ),
        "socket.raw_read" => Some(
            "  (import \"emela_socket\" \"raw_read\" (func $host_socket_raw_read (param i32 i32) (result i32)))\n",
        ),
        "socket.raw_write" => Some(
            "  (import \"emela_socket\" \"raw_write\" (func $host_socket_raw_write (param i32 i32) (result i32)))\n",
        ),
        "socket.raw_close" => Some(
            "  (import \"emela_socket\" \"raw_close\" (func $host_socket_raw_close (param i32) (result i32)))\n",
        ),
        _ => None,
    }
}

/// The wasm function id for a platform function's runtime glue.
fn platform_wasm_name(canonical: &str) -> String {
    let mut out = String::from("$plat_");
    for ch in canonical.chars() {
        out.push(if ch.is_ascii_alphanumeric() { ch } else { '_' });
    }
    out
}

/// The native instruction an intrinsic (spec 0021) inlines to, or `None` if this
/// backend does not provide it.
/// Whether the wasm backend can inline `name`: either a single-instruction
/// intrinsic or a structural one emitted by a dedicated helper (spec 0021).
fn wasm_provides_intrinsic(name: &str) -> bool {
    // A single-instruction intrinsic, or a structural one emitted by a helper
    // (`string_concat`, `string_from_char`, `array_*`) or a shared runtime
    // function (`string_eq`/`string_lt`, `string_length`/`string_char_at`/
    // `string_slice`). `char_code` / `char_from_code` are the identity on the
    // `Char` representation (its i32 code point).
    intrinsic_wasm(name).is_some()
        || matches!(
            name,
            "string_concat"
                | "string_eq"
                | "string_lt"
                | "string_length"
                | "string_char_at"
                | "string_slice"
                | "char_code"
                | "char_from_code"
                | "string_from_char"
                | "bytes_length"
                | "bytes_get_unchecked"
                | "bytes_slice"
                | "bytes_concat"
                | "bytes_eq"
                | "bytes_from_string"
                | "bytes_as_string_unchecked"
                | "array_length"
                | "array_get_unchecked"
                | "array_push"
        )
}

fn intrinsic_wasm(name: &str) -> Option<&'static str> {
    Some(match name {
        "i32_add" => "i32.add",
        "i32_sub" => "i32.sub",
        "i32_mul" => "i32.mul",
        "i32_div_s" => "i32.div_s",
        "i32_rem_s" => "i32.rem_s",
        "i32_eq" => "i32.eq",
        "i32_lt_s" => "i32.lt_s",
        "f64_add" => "f64.add",
        "f64_sub" => "f64.sub",
        "f64_mul" => "f64.mul",
        "f64_div" => "f64.div",
        "f64_eq" => "f64.eq",
        "f64_lt" => "f64.lt",
        "f64_sqrt" => "f64.sqrt",
        _ => return None,
    })
}

/// WASI-backed glue for a platform function, or `None` if not provided.
///
/// `write_*` take a string pointer to `[len: i32][utf8]`. The iovec/nwritten
/// scratch lives in the null-guard region `[0, 16)`.
fn platform_glue(canonical: &str) -> Option<&'static str> {
    match canonical {
        "io.write_stdout" => Some(WRITE_STDOUT_GLUE),
        "io.write_stderr" => Some(WRITE_STDERR_GLUE),
        "http.request" => Some(HTTP_REQUEST_GLUE),
        "socket.raw_listen" => Some(SOCKET_RAW_LISTEN_GLUE),
        "socket.raw_accept" => Some(SOCKET_RAW_ACCEPT_GLUE),
        "socket.raw_read" => Some(SOCKET_RAW_READ_GLUE),
        "socket.raw_write" => Some(SOCKET_RAW_WRITE_GLUE),
        "socket.raw_close" => Some(SOCKET_RAW_CLOSE_GLUE),
        _ => None,
    }
}

/// `http.request` (spec 0044) passes the guest `Request` pointer to the host,
/// which reads it from linear memory, performs the exchange, and returns a
/// spec-0043 Result cell (`[ok][pad][Response | HttpError]`) it allocated in
/// guest memory via the exported `alloc`.
const HTTP_REQUEST_GLUE: &str = "  (func $plat_http_request (param $req i32) (result i32)\n    local.get $req\n    call $host_http_request)\n";

// The Socket operations (spec 0050) forward their arguments to the host, which
// returns a spec-0043 Result cell it allocated in guest memory (via `alloc`).
// `raw_close` is infallible: it forwards through and yields Unit (the host
// returns 0).
const SOCKET_RAW_LISTEN_GLUE: &str = "  (func $plat_socket_raw_listen (param $port i32) (result i32)\n    local.get $port\n    call $host_socket_raw_listen)\n";

const SOCKET_RAW_ACCEPT_GLUE: &str = "  (func $plat_socket_raw_accept (param $listener i32) (result i32)\n    local.get $listener\n    call $host_socket_raw_accept)\n";

const SOCKET_RAW_READ_GLUE: &str = "  (func $plat_socket_raw_read (param $conn i32) (param $max i32) (result i32)\n    local.get $conn\n    local.get $max\n    call $host_socket_raw_read)\n";

const SOCKET_RAW_WRITE_GLUE: &str = "  (func $plat_socket_raw_write (param $conn i32) (param $data i32) (result i32)\n    local.get $conn\n    local.get $data\n    call $host_socket_raw_write)\n";

const SOCKET_RAW_CLOSE_GLUE: &str = "  (func $plat_socket_raw_close (param $handle i32) (result i32)\n    local.get $handle\n    call $host_socket_raw_close)\n";

// The Socket operations (spec 0050) forward their arguments to the host, which
// returns a spec-0043 Result cell it allocated in guest memory (via `alloc`).
// `raw_close` is infallible: it forwards through and yields Unit (the host
// returns 0).
const SOCKET_RAW_LISTEN_GLUE: &str = "  (func $plat_socket_raw_listen (param $port i32) (result i32)\n    local.get $port\n    call $host_socket_raw_listen)\n";

const SOCKET_RAW_ACCEPT_GLUE: &str = "  (func $plat_socket_raw_accept (param $listener i32) (result i32)\n    local.get $listener\n    call $host_socket_raw_accept)\n";

const SOCKET_RAW_READ_GLUE: &str = "  (func $plat_socket_raw_read (param $conn i32) (param $max i32) (result i32)\n    local.get $conn\n    local.get $max\n    call $host_socket_raw_read)\n";

const SOCKET_RAW_WRITE_GLUE: &str = "  (func $plat_socket_raw_write (param $conn i32) (param $data i32) (result i32)\n    local.get $conn\n    local.get $data\n    call $host_socket_raw_write)\n";

const SOCKET_RAW_CLOSE_GLUE: &str = "  (func $plat_socket_raw_close (param $handle i32) (result i32)\n    local.get $handle\n    call $host_socket_raw_close)\n";

const WRITE_STDOUT_GLUE: &str = "  (func $plat_io_write_stdout (param $s i32) (result i32)\n    i32.const 0\n    local.get $s\n    i32.const 4\n    i32.add\n    i32.store\n    i32.const 4\n    local.get $s\n    i32.load\n    i32.store\n    i32.const 1\n    i32.const 0\n    i32.const 1\n    i32.const 8\n    call $wasi_fd_write\n    drop\n    i32.const 0)\n";

const WRITE_STDERR_GLUE: &str = "  (func $plat_io_write_stderr (param $s i32) (result i32)\n    i32.const 0\n    local.get $s\n    i32.const 4\n    i32.add\n    i32.store\n    i32.const 4\n    local.get $s\n    i32.load\n    i32.store\n    i32.const 2\n    i32.const 0\n    i32.const 1\n    i32.const 8\n    call $wasi_fd_write\n    drop\n    i32.const 0)\n";

fn emit_types(sigs: &SigTable) -> String {
    let mut out = String::new();
    for (index, sig) in sigs.list.iter().enumerate() {
        out.push_str(&format!("  (type $sig_{index} (func"));
        for param in &sig.params {
            let _ = write!(out, " (param {})", param.keyword());
        }
        let _ = writeln!(out, " (result {})))", sig.result.keyword());
    }
    // The drop functions `$rc_release` dispatches through the table.
    out.push_str("  (type $drop_t (func (param i32)))\n");
    out
}

fn emit_table(table: &FnTable, drops: &DropTable) -> String {
    let total = table.n_top + table.lambdas.len() as u32 + drops.names.len() as u32;
    let mut entries: Vec<String> = vec![String::new(); total as usize];
    for (name, index) in &table.toplevel {
        entries[*index as usize] = format!("$f_{name}");
    }
    for (offset, _) in table.lambdas.iter().enumerate() {
        entries[table.n_top as usize + offset] = format!("$lambda_{offset}");
    }
    for (offset, name) in drops.names.iter().enumerate() {
        entries[drops.base as usize + offset] = name.clone();
    }
    let mut out = String::new();
    let _ = writeln!(out, "  (table {total} funcref)");
    let _ = writeln!(out, "  (elem (i32.const 0) {})", entries.join(" "));
    out
}

/// The memory-management runtime (spec 0048 Compilation Notes).
///
/// Every heap block is `[drop_idx: i32][refcount: i32][payload...]` and object
/// pointers address the *payload*, so all payload layouts keep their historical
/// offsets (the header lives at negative offsets). Blocks are 8-aligned and
/// their total size (header included) is a multiple of 8; `$free` takes that
/// total. A free block reuses its first two words as `[size][next]` on a
/// single first-fit, splitting free list.
///
/// `$rc_retain` / `$rc_release` ignore pointers below `heap_base` (static
/// string literals, spec 0048 A6). At refcount zero, `$rc_release` dispatches
/// the block's drop function through the function table (`drop_idx`), which
/// releases the children and calls `$free` with the computed total size.
///
/// `$alloc_host` keeps the one-parameter allocator shape the HTTP host binds
/// against. Host-built values get the `leak_idx` drop entry: only the host
/// knows their shape, so a released one stays allocated (a follow-up will add
/// a host shape protocol; before ARC, *everything* leaked).
fn runtime_wat(heap_base: u32, leak_idx: u32) -> String {
    format!(
        r#"  (func $alloc (param $n i32) (param $drop i32) (result i32)
    (local $total i32) (local $prev i32) (local $cur i32) (local $sz i32) (local $rest i32) (local $p i32) (local $end i32)
    local.get $n i32.const 15 i32.add i32.const -8 i32.and local.set $total
    i32.const 0 local.set $prev
    global.get $free_list local.set $cur
    block $miss
      loop $scan
        local.get $cur i32.eqz br_if $miss
        local.get $cur i32.load local.set $sz
        local.get $sz local.get $total i32.ge_u
        if
          local.get $sz local.get $total i32.sub local.set $rest
          local.get $rest i32.const 8 i32.ge_u
          if
            local.get $cur local.get $total i32.add local.set $p
            local.get $p local.get $rest i32.store
            local.get $p local.get $cur i32.load offset=4 i32.store offset=4
            local.get $prev
            if
              local.get $prev local.get $p i32.store offset=4
            else
              local.get $p global.set $free_list
            end
          else
            local.get $prev
            if
              local.get $prev local.get $cur i32.load offset=4 i32.store offset=4
            else
              local.get $cur i32.load offset=4 global.set $free_list
            end
          end
          local.get $cur local.get $drop i32.store
          local.get $cur i32.const 1 i32.store offset=4
          global.get $live local.get $total i32.add global.set $live
          local.get $cur i32.const 8 i32.add
          return
        end
        local.get $cur local.set $prev
        local.get $cur i32.load offset=4 local.set $cur
        br $scan
      end
    end
    global.get $heap i32.const 7 i32.add i32.const -8 i32.and local.set $p
    local.get $p local.get $total i32.add local.set $end
    local.get $end memory.size i32.const 16 i32.shl i32.gt_u
    if
      local.get $end memory.size i32.const 16 i32.shl i32.sub
      i32.const 65535 i32.add i32.const 16 i32.shr_u
      memory.grow
      i32.const -1 i32.eq
      if unreachable end
    end
    local.get $end global.set $heap
    local.get $p local.get $drop i32.store
    local.get $p i32.const 1 i32.store offset=4
    global.get $live local.get $total i32.add global.set $live
    local.get $p i32.const 8 i32.add)
  (func $alloc_host (param $n i32) (result i32)
    local.get $n
    i32.const {leak_idx}
    call $alloc)
  (func $free (param $p i32) (param $size i32)
    (local $blk i32)
    local.get $p i32.const 8 i32.sub local.set $blk
    local.get $blk local.get $size i32.store
    local.get $blk global.get $free_list i32.store offset=4
    local.get $blk global.set $free_list
    global.get $live local.get $size i32.sub global.set $live)
  (func $rc_retain (param $p i32) (result i32)
    local.get $p i32.const {heap_base} i32.lt_u
    if local.get $p return end
    local.get $p i32.const 4 i32.sub
    local.get $p i32.const 4 i32.sub i32.load
    i32.const 1 i32.add
    i32.store
    local.get $p)
  (func $rc_release (param $p i32)
    (local $rc i32)
    local.get $p i32.const {heap_base} i32.lt_u
    if return end
    local.get $p i32.const 4 i32.sub i32.load i32.const 1 i32.sub local.set $rc
    local.get $rc
    if
      local.get $p i32.const 4 i32.sub local.get $rc i32.store
      return
    end
    local.get $p
    local.get $p i32.const 8 i32.sub i32.load
    call_indirect (type $drop_t))
"#
    )
}

/// Runtime helpers for `Eq`/`Ord for String` (spec 0027). A string is
/// `[len: i32][utf8 bytes]`, so `$string_eq` compares lengths then bytes, and
/// `$string_lt` walks the shared prefix and orders by the first differing byte
/// (then by length) — lexicographic over bytes, i.e. code-point order. Both
/// return an `i32` boolean. They are always emitted; an unused function is fine.
const STRING_CMP: &str = r#"  (func $string_eq (param $a i32) (param $b i32) (result i32)
    (local $la i32) (local $lb i32) (local $i i32)
    local.get $a i32.load local.set $la
    local.get $b i32.load local.set $lb
    local.get $la local.get $lb i32.ne
    if i32.const 0 return end
    i32.const 0 local.set $i
    block $done
      loop $loop
        local.get $i local.get $la i32.ge_s br_if $done
        local.get $a i32.const 4 i32.add local.get $i i32.add i32.load8_u
        local.get $b i32.const 4 i32.add local.get $i i32.add i32.load8_u
        i32.ne
        if i32.const 0 return end
        local.get $i i32.const 1 i32.add local.set $i
        br $loop
      end
    end
    i32.const 1)
  (func $string_lt (param $a i32) (param $b i32) (result i32)
    (local $la i32) (local $lb i32) (local $i i32) (local $n i32) (local $ca i32) (local $cb i32)
    local.get $a i32.load local.set $la
    local.get $b i32.load local.set $lb
    local.get $la local.get $lb i32.lt_s
    if (result i32) local.get $la else local.get $lb end
    local.set $n
    i32.const 0 local.set $i
    block $done
      loop $loop
        local.get $i local.get $n i32.ge_s br_if $done
        local.get $a i32.const 4 i32.add local.get $i i32.add i32.load8_u local.set $ca
        local.get $b i32.const 4 i32.add local.get $i i32.add i32.load8_u local.set $cb
        local.get $ca local.get $cb i32.lt_u
        if i32.const 1 return end
        local.get $ca local.get $cb i32.gt_u
        if i32.const 0 return end
        local.get $i i32.const 1 i32.add local.set $i
        br $loop
      end
    end
    local.get $la local.get $lb i32.lt_s)
"#;

/// Runtime helpers for the scalar string operations (spec 0030). A string is
/// `[len: i32 (byte length)][utf8 bytes]`; these count/index/slice in Unicode
/// scalar (code point) units, never bytes. `$utf8_seqlen` maps a lead byte to
/// its sequence length. They are always emitted; an unused function is fine.
const STRING_INDEX: &str = r#"  (func $utf8_seqlen (param $b i32) (result i32)
    local.get $b i32.const 240 i32.ge_u if i32.const 4 return end
    local.get $b i32.const 224 i32.ge_u if i32.const 3 return end
    local.get $b i32.const 192 i32.ge_u if i32.const 2 return end
    i32.const 1)
  (func $string_length (param $s i32) (result i32)
    (local $len i32) (local $i i32) (local $count i32)
    local.get $s i32.load local.set $len
    i32.const 0 local.set $i
    i32.const 0 local.set $count
    block $done
      loop $loop
        local.get $i local.get $len i32.ge_s br_if $done
        local.get $s i32.const 4 i32.add local.get $i i32.add i32.load8_u
        i32.const 192 i32.and i32.const 128 i32.ne
        if local.get $count i32.const 1 i32.add local.set $count end
        local.get $i i32.const 1 i32.add local.set $i
        br $loop
      end
    end
    local.get $count)
  (func $string_byte_offset (param $s i32) (param $k i32) (result i32)
    (local $base i32) (local $p i32)
    local.get $s i32.const 4 i32.add local.set $base
    i32.const 0 local.set $p
    block $done
      loop $loop
        local.get $k i32.const 0 i32.le_s br_if $done
        local.get $base local.get $p i32.add i32.load8_u call $utf8_seqlen
        local.get $p i32.add local.set $p
        local.get $k i32.const 1 i32.sub local.set $k
        br $loop
      end
    end
    local.get $p)
  (func $string_char_at (param $s i32) (param $idx i32) (result i32)
    (local $base i32) (local $p i32) (local $b i32) (local $n i32) (local $cp i32) (local $j i32)
    local.get $s i32.const 4 i32.add local.set $base
    local.get $s local.get $idx call $string_byte_offset local.set $p
    local.get $base local.get $p i32.add i32.load8_u local.set $b
    local.get $b call $utf8_seqlen local.set $n
    local.get $n i32.const 1 i32.eq
    if local.get $b return end
    local.get $b i32.const 127 local.get $n i32.shr_u i32.and local.set $cp
    i32.const 1 local.set $j
    block $done
      loop $loop
        local.get $j local.get $n i32.ge_s br_if $done
        local.get $cp i32.const 6 i32.shl
        local.get $base local.get $p i32.add local.get $j i32.add i32.load8_u i32.const 63 i32.and
        i32.or
        local.set $cp
        local.get $j i32.const 1 i32.add local.set $j
        br $loop
      end
    end
    local.get $cp)
  (func $string_slice (param $s i32) (param $start i32) (param $end i32) (result i32)
    (local $len i32) (local $sb i32) (local $eb i32) (local $n i32) (local $out i32)
    local.get $s call $string_length local.set $len
    local.get $start i32.const 0 i32.lt_s if i32.const 0 local.set $start end
    local.get $start local.get $len i32.gt_s if local.get $len local.set $start end
    local.get $end i32.const 0 i32.lt_s if i32.const 0 local.set $end end
    local.get $end local.get $len i32.gt_s if local.get $len local.set $end end
    local.get $start local.get $end i32.ge_s
    if
      i32.const 4 i32.const 0 call $alloc local.set $out
      local.get $out i32.const 0 i32.store
      local.get $out return
    end
    local.get $s local.get $start call $string_byte_offset local.set $sb
    local.get $s local.get $end call $string_byte_offset local.set $eb
    local.get $eb local.get $sb i32.sub local.set $n
    i32.const 4 local.get $n i32.add i32.const 0 call $alloc local.set $out
    local.get $out local.get $n i32.store
    local.get $out i32.const 4 i32.add
    local.get $s i32.const 4 i32.add local.get $sb i32.add
    local.get $n
    memory.copy
    local.get $out)
  (func $string_from_char (param $code i32) (result i32)
    (local $out i32)
    local.get $code i32.const 128 i32.lt_u
    if
      i32.const 5 i32.const 0 call $alloc local.set $out
      local.get $out i32.const 1 i32.store
      local.get $out i32.const 4 i32.add local.get $code i32.store8
      local.get $out return
    end
    local.get $code i32.const 2048 i32.lt_u
    if
      i32.const 6 i32.const 0 call $alloc local.set $out
      local.get $out i32.const 2 i32.store
      local.get $out i32.const 4 i32.add
        local.get $code i32.const 6 i32.shr_u i32.const 192 i32.or i32.store8
      local.get $out i32.const 5 i32.add
        local.get $code i32.const 63 i32.and i32.const 128 i32.or i32.store8
      local.get $out return
    end
    local.get $code i32.const 65536 i32.lt_u
    if
      i32.const 7 i32.const 0 call $alloc local.set $out
      local.get $out i32.const 3 i32.store
      local.get $out i32.const 4 i32.add
        local.get $code i32.const 12 i32.shr_u i32.const 224 i32.or i32.store8
      local.get $out i32.const 5 i32.add
        local.get $code i32.const 6 i32.shr_u i32.const 63 i32.and i32.const 128 i32.or i32.store8
      local.get $out i32.const 6 i32.add
        local.get $code i32.const 63 i32.and i32.const 128 i32.or i32.store8
      local.get $out return
    end
    i32.const 8 i32.const 0 call $alloc local.set $out
    local.get $out i32.const 4 i32.store
    local.get $out i32.const 4 i32.add
      local.get $code i32.const 18 i32.shr_u i32.const 240 i32.or i32.store8
    local.get $out i32.const 5 i32.add
      local.get $code i32.const 12 i32.shr_u i32.const 63 i32.and i32.const 128 i32.or i32.store8
    local.get $out i32.const 6 i32.add
      local.get $code i32.const 6 i32.shr_u i32.const 63 i32.and i32.const 128 i32.or i32.store8
    local.get $out i32.const 7 i32.add
      local.get $code i32.const 63 i32.and i32.const 128 i32.or i32.store8
    local.get $out)
"#;

/// [`STRING_INDEX`] with the string shape's drop index stamped into its
/// allocation sites (spec 0048).
fn string_index_wat(string_drop: u32) -> String {
    STRING_INDEX.replace(
        "i32.const 0 call $alloc",
        &format!("i32.const {string_drop} call $alloc"),
    )
}

/// Runtime helper for the byte-unit slice (spec 0051). A `Bytes` is
/// `[len: i32 (byte length)][bytes]`; unlike `$string_slice`, `start`/`end` are
/// byte offsets directly (no scalar walk). Out-of-range bounds are clamped.
/// Allocated `Bytes` share the string drop shape (spec 0048). Always emitted;
/// an unused function is fine.
const BYTES_HELPERS: &str = r#"  (func $bytes_slice (param $s i32) (param $start i32) (param $end i32) (result i32)
    (local $len i32) (local $n i32) (local $out i32)
    local.get $s i32.load local.set $len
    local.get $start i32.const 0 i32.lt_s if i32.const 0 local.set $start end
    local.get $start local.get $len i32.gt_s if local.get $len local.set $start end
    local.get $end i32.const 0 i32.lt_s if i32.const 0 local.set $end end
    local.get $end local.get $len i32.gt_s if local.get $len local.set $end end
    local.get $start local.get $end i32.ge_s
    if
      i32.const 4 i32.const 0 call $alloc local.set $out
      local.get $out i32.const 0 i32.store
      local.get $out return
    end
    local.get $end local.get $start i32.sub local.set $n
    i32.const 4 local.get $n i32.add i32.const 0 call $alloc local.set $out
    local.get $out local.get $n i32.store
    local.get $out i32.const 4 i32.add
    local.get $s i32.const 4 i32.add local.get $start i32.add
    local.get $n
    memory.copy
    local.get $out)
  (func $blob_dup (param $s i32) (result i32)
    (local $len i32) (local $out i32)
    local.get $s i32.load local.set $len
    i32.const 4 local.get $len i32.add i32.const 0 call $alloc local.set $out
    local.get $out local.get $len i32.store
    local.get $out i32.const 4 i32.add
    local.get $s i32.const 4 i32.add
    local.get $len
    memory.copy
    local.get $out)
"#;

/// [`BYTES_HELPERS`] with the (shared string) drop index stamped into its
/// allocation sites (spec 0048).
fn bytes_helpers_wat(string_drop: u32) -> String {
    BYTES_HELPERS.replace(
        "i32.const 0 call $alloc",
        &format!("i32.const {string_drop} call $alloc"),
    )
}

fn emit_start(main: &IrFunction) -> String {
    let mut out = String::new();
    out.push_str("  (func $_start\n");
    out.push_str("    i32.const 0\n");
    out.push_str("    call $f_main\n");
    if main.ret == Type::Int {
        // The Int result is the exit code.
        out.push_str("    call $proc_exit)\n");
    } else if is_heap(&main.ret) {
        // `main` hands over an owned heap result (spec 0048): release it so a
        // clean run ends with `live_bytes` at exactly zero, then exit 0.
        out.push_str("    call $rc_release\n    i32.const 0\n    call $proc_exit)\n");
    } else {
        // Drop any other result and exit 0.
        out.push_str("    drop\n    i32.const 0\n    call $proc_exit)\n");
    }
    out
}

fn emit_function(function: &IrFunction, ctx: &Ctx) -> Result<String> {
    emit_fn_like(
        &format!("$f_{}", function.name),
        &function.params,
        &function.ret,
        &function.throws,
        &[],
        &function.body,
        ctx,
    )
}

fn emit_lambda(lambda: &IrExpr, ctx: &Ctx) -> Result<String> {
    let index = ctx.table.lambda_index[&(lambda as *const IrExpr)];
    let name = format!("$lambda_{}", index - ctx.table.n_top);
    let IrExpr::Fn {
        params,
        ret,
        throws,
        captures,
        body,
        ..
    } = lambda
    else {
        unreachable!("lambda table only holds Fn nodes");
    };
    emit_fn_like(&name, params, ret, throws, captures, body, ctx)
}

fn emit_fn_like(
    wasm_name: &str,
    params: &[IrParam],
    ret: &Type,
    throws: &Option<Type>,
    captures: &[IrCapture],
    body: &IrExpr,
    ctx: &Ctx,
) -> Result<String> {
    let mut emitter = FnEmitter::new(ctx);
    // Which bindings the RC pass releases (nested lambdas' names are globally
    // unique after alpha-renaming, so the over-collection is harmless).
    walk(body, &mut |expr| {
        if let IrExpr::Release { name, .. } = expr {
            emitter.released.insert(name.clone());
        }
    });
    for (index, param) in params.iter().enumerate() {
        // Param 0 is the closure environment; user params follow.
        let local = format!("$p{}", index + 1);
        if is_heap(&param.ty) {
            emitter.managed.push((param.name.clone(), local.clone()));
        }
        emitter.bind(param.name.clone(), Slot::Local(local));
    }
    for (name, offset, ty) in capture_layout(captures) {
        emitter.bind(name, Slot::Capture(offset, ty));
    }
    // A body with self-tail-calls (spec 0045) runs inside a `loop`: the call
    // reassigns `$p1..$pN` and branches back; normal completion falls out of
    // the loop with the body's value.
    let tail_loop = contains_tail_self_call(body);
    if tail_loop {
        emitter.line(&format!(
            "(loop $tail (result {})",
            WasmTy::of(ret).keyword()
        ));
    }
    emitter.emit(body)?;
    if tail_loop {
        emitter.line(")");
    }
    if throws.is_some() {
        // The body left its success value on the stack; wrap it as `Ok`.
        emitter.wrap_ok(ret);
    }

    let mut signature = format!("  (func {wasm_name} (param $p0 i32)");
    for (index, param) in params.iter().enumerate() {
        let _ = write!(
            signature,
            " (param $p{} {})",
            index + 1,
            WasmTy::of(&param.ty).keyword()
        );
    }
    let _ = writeln!(
        signature,
        " (result {})",
        result_wasm_ty(ret, throws.is_some()).keyword()
    );

    let mut out = signature;
    for (id, ty) in &emitter.locals {
        let _ = writeln!(out, "    (local {} {})", id, ty.keyword());
    }
    out.push_str(&emitter.code);
    out.push_str(")\n");
    Ok(out)
}

// ---------------------------------------------------------------------------
// Expression emission
// ---------------------------------------------------------------------------

struct Ctx<'a> {
    table: &'a FnTable<'a>,
    sigs: &'a SigTable,
    strings: &'a StringTable,
    drops: &'a DropTable,
}

/// Where a bound name lives at run time.
enum Slot {
    /// A wasm local (a parameter or a `let` binding).
    Local(String),
    /// A captured variable, at `offset` bytes into the environment pointer.
    Capture(u32, WasmTy),
}

struct FnEmitter<'a> {
    code: String,
    locals: Vec<(String, WasmTy)>,
    scope: Vec<(String, Slot)>,
    counter: usize,
    /// Labels of the enclosing `try` blocks' catch handlers. A `throw` or
    /// throwing call branches to the innermost one, or returns `Err` if empty.
    catch_stack: Vec<String>,
    /// Binding names the RC pass releases somewhere (`Release` nodes).
    released: HashSet<String>,
    /// `$rc` transfer temporaries: heap-typed, not release-named, consumed by
    /// their single read. Reading one zeroes its local (spec 0048 A7).
    transfer_heap: HashSet<String>,
    /// Every heap-holding local in binding order (heap params, heap `let`s,
    /// caught errors), for unwind cleanup: a release obligation not yet met is
    /// exactly a non-zero entry, because every release/consumption zeroes its
    /// local. `rc_release(0)` is a no-op, so cleanup needs no branches.
    managed: Vec<(String, String)>,
    ctx: &'a Ctx<'a>,
}

impl<'a> FnEmitter<'a> {
    fn new(ctx: &'a Ctx<'a>) -> Self {
        Self {
            code: String::new(),
            locals: Vec::new(),
            scope: Vec::new(),
            counter: 0,
            catch_stack: Vec::new(),
            released: HashSet::new(),
            transfer_heap: HashSet::new(),
            managed: Vec::new(),
            ctx,
        }
    }

    /// Releases (and zeroes) the managed locals from `from` on: the unwind
    /// cleanup at a catch entry (`from` = the mark before the try body) or a
    /// function-boundary exit (`from` = 0). Zeroed entries no-op.
    fn release_live_managed(&mut self, from: usize) {
        let locals: Vec<String> = self.managed[from..]
            .iter()
            .map(|(_, local)| local.clone())
            .collect();
        for local in locals {
            self.line(&format!("local.get {local}"));
            self.line("call $rc_release");
            self.line("i32.const 0");
            self.line(&format!("local.set {local}"));
        }
    }

    fn bind(&mut self, name: String, slot: Slot) {
        self.scope.push((name, slot));
    }

    fn slot(&self, name: &str) -> Option<&Slot> {
        self.scope
            .iter()
            .rev()
            .find(|(bound, _)| bound == name)
            .map(|(_, slot)| slot)
    }

    fn fresh_local(&mut self, ty: WasmTy) -> String {
        let id = format!("$v{}", self.counter);
        self.counter += 1;
        self.locals.push((id.clone(), ty));
        id
    }

    fn line(&mut self, instruction: &str) {
        self.code.push_str("    ");
        self.code.push_str(instruction);
        self.code.push('\n');
    }

    /// Calls `$alloc` for the block size on the stack, stamping the shape's
    /// drop index (spec 0048) into the block header.
    fn call_alloc(&mut self, drop_idx: u32) {
        self.line(&format!("i32.const {drop_idx}"));
        self.line("call $alloc");
    }

    fn emit(&mut self, expr: &IrExpr) -> Result<()> {
        match expr {
            IrExpr::Int(value) => self.line(&format!("i32.const {value}")),
            IrExpr::Bool(value) => self.line(&format!("i32.const {}", i32::from(*value))),
            IrExpr::Unit => self.line("i32.const 0"),
            IrExpr::Float(value) => self.line(&format!("f64.const {value}")),
            IrExpr::Var { name, .. } => self.emit_var(name)?,
            IrExpr::Binary {
                op,
                ty,
                left,
                right,
                ..
            } => {
                self.emit(left)?;
                self.emit(right)?;
                self.line(binary_op(*op, ty));
            }
            IrExpr::Let {
                name,
                value_ty,
                value,
                next,
            } => {
                self.emit(value)?;
                let id = self.fresh_local(WasmTy::of(value_ty));
                self.line(&format!("local.set {id}"));
                if is_heap(value_ty) {
                    self.managed.push((name.clone(), id.clone()));
                    if name.starts_with("$rc") && !self.released.contains(name) {
                        self.transfer_heap.insert(name.clone());
                    }
                }
                self.bind(name.clone(), Slot::Local(id));
                self.emit(next)?;
            }
            IrExpr::Call { callee, args, ret } => {
                self.emit_call(callee, args)?;
                if is_throwing(callee) {
                    // A throwing call yields a Result pointer; unwrap it.
                    self.unwrap_result(ret)?;
                }
            }
            IrExpr::Platform {
                name,
                args,
                ret,
                throws,
            } => {
                for arg in args {
                    self.emit(arg)?;
                }
                self.line(&format!("call {}", platform_wasm_name(name)));
                if throws.is_some() {
                    // A fallible platform function (spec 0043) returns the same
                    // Result representation as a throwing Emela function.
                    self.unwrap_result(ret)?;
                }
            }
            // An intrinsic (spec 0021) inlines to a native instruction, or, for a
            // structural one like `string_concat`, to a dedicated helper.
            IrExpr::Intrinsic { name, args, ret } => match name.as_str() {
                "string_concat" => self.emit_concat(&args[0], &args[1])?,
                // String comparison calls the shared runtime helper, which walks
                // the `[len][utf8]` bytes (see `STRING_CMP`).
                "string_eq" | "string_lt" => {
                    self.emit(&args[0])?;
                    self.emit(&args[1])?;
                    self.line(&format!("call ${name}"));
                }
                // Scalar string operations (spec 0030) call the shared runtime
                // helpers that walk the UTF-8 bytes in code-point units (see
                // `STRING_INDEX`).
                "string_length" | "string_char_at" | "string_slice" => {
                    for arg in args {
                        self.emit(arg)?;
                    }
                    self.line(&format!("call ${name}"));
                }
                // A `Char` is already its i32 code point (spec 0017), so
                // `char_code` and `char_from_code` are the identity on that
                // representation. `string_from_char` builds the `[len][utf8]`.
                "char_code" | "char_from_code" => self.emit(&args[0])?,
                "string_from_char" => self.emit_string_from_char(&args[0])?,
                // Byte-sequence operations (spec 0051). `Bytes` shares `String`'s
                // `[len][bytes]` representation, so `bytes_concat` reuses the
                // string concat helper, `bytes_eq` the `$string_eq` byte walk,
                // and `bytes_from_string` is the identity. `bytes_length` is the
                // raw byte length (`i32.load`) and `bytes_get_unchecked` a raw
                // byte load — both differ from the scalar-counting string ops.
                "bytes_concat" => self.emit_concat(&args[0], &args[1])?,
                "bytes_eq" => {
                    self.emit(&args[0])?;
                    self.emit(&args[1])?;
                    self.line("call $string_eq");
                }
                // `bytes_from_string` (spec 0051 B6) / `bytes_as_string_unchecked`
                // (spec 0051 B7) *copy* rather than aliasing their argument. The
                // representation is shared, so an identity would return the arg
                // pointer — but the RC pass (spec 0048) treats an intrinsic result
                // as a fresh allocation and its argument as a borrow (rc.rs), so an
                // alias to a heap argument is released twice (double free, seen as a
                // drop-dispatch `indirect call type mismatch`). `$blob_dup` gives the
                // result its own `[len][bytes]` block.
                "bytes_from_string" | "bytes_as_string_unchecked" => {
                    self.emit(&args[0])?;
                    self.line("call $blob_dup");
                }
                "bytes_length" => {
                    self.emit(&args[0])?;
                    self.line("i32.load");
                }
                "bytes_get_unchecked" => {
                    self.emit(&args[0])?;
                    self.line("i32.const 4");
                    self.line("i32.add");
                    self.emit(&args[1])?;
                    self.line("i32.add");
                    self.line("i32.load8_u");
                }
                "bytes_slice" => {
                    for arg in args {
                        self.emit(arg)?;
                    }
                    self.line("call $bytes_slice");
                }
                // Array operations (spec 0007), formerly the `ArrayLength` /
                // `ArrayGet` / `ArrayPush` nodes. The element type is recovered
                // from the monomorphized intrinsic's types: `array_get_unchecked`'s
                // is the return type `ret`, `array_push`'s is the element of `ret`.
                "array_length" => {
                    self.emit(&args[0])?;
                    self.line("i32.load");
                }
                "array_get_unchecked" => self.emit_array_get(&args[0], &args[1], ret)?,
                "array_push" => {
                    let Type::Array(elem_ty) = ret else {
                        return Err(BackendError::new(
                            "internal error: `array_push` return type is not an array",
                        ));
                    };
                    self.emit_array_push(&args[0], &args[1], elem_ty)?;
                }
                _ => {
                    for arg in args {
                        self.emit(arg)?;
                    }
                    let instruction = intrinsic_wasm(name).ok_or_else(|| {
                        BackendError::new(format!(
                            "backend `wasm-wasi` does not provide intrinsic `{name}`"
                        ))
                    })?;
                    self.line(instruction);
                }
            },
            IrExpr::String(value) => {
                let offset = *self.ctx.strings.offsets.get(value).ok_or_else(|| {
                    BackendError::new("internal error: string literal was not interned")
                })?;
                self.line(&format!("i32.const {offset}"));
            }
            // A `Char` is its codepoint as i32 (spec 0017).
            IrExpr::Char(code) => self.line(&format!("i32.const {code}")),
            IrExpr::Concat { left, right } => self.emit_concat(left, right)?,
            IrExpr::Array { elem_ty, elems } => self.emit_array(elem_ty, elems)?,
            IrExpr::FunctionRef { name, .. } => self.emit_function_ref(name)?,
            IrExpr::Fn { captures, .. } => self.emit_closure(expr, captures)?,
            IrExpr::EnumValue {
                ty, tag, payload, ..
            } => {
                let drop_idx = self.ctx.drops.of(&DropKey::Enum(mangle(ty)));
                self.emit_enum_value(drop_idx, *tag, payload)?;
            }
            IrExpr::RecordValue { fields, .. } => self.emit_record_value(fields)?,
            IrExpr::FieldAccess {
                target,
                index,
                field_ty,
            } => {
                self.emit(target)?;
                self.line(&format!("i32.const {}", index * 8));
                self.line("i32.add");
                self.line(WasmTy::of(field_ty).load());
                // A loaded heap field leaves the borrowed record as an owned
                // value (spec 0048).
                if is_heap(field_ty) {
                    self.line("call $rc_retain");
                }
            }
            IrExpr::Match {
                scrutinee,
                arms,
                ty,
            } => self.emit_match(scrutinee, arms, ty)?,
            IrExpr::If {
                cond,
                then,
                els,
                ty,
            } => {
                self.emit(cond)?;
                self.line(&format!("if (result {})", WasmTy::of(ty).keyword()));
                self.emit(then)?;
                self.line("else");
                self.emit(els)?;
                self.line("end");
            }
            IrExpr::Panic { .. } => self.line("unreachable"),
            IrExpr::Throw { value } => {
                self.emit(value)?;
                self.raise_error();
            }
            IrExpr::Try {
                body,
                arms,
                ty,
                err_name,
            } => self.emit_try(body, arms, ty, err_name.as_deref())?,
            IrExpr::Question { value, .. } => self.emit_question(value)?,
            IrExpr::TailSelfCall { args, .. } => self.emit_tail_self_call(args)?,
            // RC ops (spec 0048), inserted by `emela_codegen::rc::insert_rc_ops`.
            // The pass only produces them for heap-typed (i32 pointer) values.
            IrExpr::Retain { value } => {
                self.emit(value)?;
                self.line("call $rc_retain");
            }
            IrExpr::Release { name, next, .. } => {
                // Meet the obligation and zero the local, so an unwind
                // cleanup that runs later cannot double-release it.
                let Some(Slot::Local(id)) = self.slot(name) else {
                    return Err(BackendError::new(format!(
                        "release of a non-local binding `{name}` in wasm"
                    )));
                };
                let id = id.clone();
                self.line(&format!("local.get {id}"));
                self.line("call $rc_release");
                self.line("i32.const 0");
                self.line(&format!("local.set {id}"));
                self.emit(next)?;
            }
        }
        Ok(())
    }

    /// A direct self-recursive call in tail position (spec 0045): evaluate the
    /// arguments left to right into temporaries, reassign the parameter locals,
    /// and jump back to the function-head `loop` — no stack growth.
    fn emit_tail_self_call(&mut self, args: &[IrExpr]) -> Result<()> {
        let mut temps = Vec::with_capacity(args.len());
        for arg in args {
            self.emit(arg)?;
            let temp = self.fresh_local(WasmTy::of(&arg.ty()));
            self.line(&format!("local.set {temp}"));
            temps.push(temp);
        }
        for (index, temp) in temps.iter().enumerate() {
            self.line(&format!("local.get {temp}"));
            self.line(&format!("local.set $p{}", index + 1));
        }
        self.line("br $tail");
        Ok(())
    }

    /// `string_from_char` (spec 0017): the `[len][utf8]` string of one scalar.
    /// The shared `$string_from_char` helper encodes the 1-4 byte UTF-8 form.
    fn emit_string_from_char(&mut self, value: &IrExpr) -> Result<()> {
        self.emit(value)?;
        self.line("call $string_from_char");
        Ok(())
    }

    /// `a ++ b` (spec 0017): allocate `[len_a+len_b][bytes_a bytes_b]` and copy.
    fn emit_concat(&mut self, left: &IrExpr, right: &IrExpr) -> Result<()> {
        let a = self.fresh_local(WasmTy::I32);
        let b = self.fresh_local(WasmTy::I32);
        let len_a = self.fresh_local(WasmTy::I32);
        let len_b = self.fresh_local(WasmTy::I32);
        let out = self.fresh_local(WasmTy::I32);
        self.emit(left)?;
        self.line(&format!("local.set {a}"));
        self.emit(right)?;
        self.line(&format!("local.set {b}"));
        self.line(&format!("local.get {a}"));
        self.line("i32.load");
        self.line(&format!("local.set {len_a}"));
        self.line(&format!("local.get {b}"));
        self.line("i32.load");
        self.line(&format!("local.set {len_b}"));
        // out = alloc(4 + len_a + len_b)
        self.line("i32.const 4");
        self.line(&format!("local.get {len_a}"));
        self.line("i32.add");
        self.line(&format!("local.get {len_b}"));
        self.line("i32.add");
        let string_drop = self.ctx.drops.of(&DropKey::String);
        self.call_alloc(string_drop);
        self.line(&format!("local.set {out}"));
        // store the combined length
        self.line(&format!("local.get {out}"));
        self.line(&format!("local.get {len_a}"));
        self.line(&format!("local.get {len_b}"));
        self.line("i32.add");
        self.line("i32.store");
        // memory.copy(dest = out+4, src = a+4, n = len_a)
        self.line(&format!("local.get {out}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {a}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {len_a}"));
        self.line("memory.copy");
        // memory.copy(dest = out+4+len_a, src = b+4, n = len_b)
        self.line(&format!("local.get {out}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {len_a}"));
        self.line("i32.add");
        self.line(&format!("local.get {b}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {len_b}"));
        self.line("memory.copy");
        self.line(&format!("local.get {out}"));
        Ok(())
    }

    /// Allocates `[tag:i32][field*8bytes]` and leaves the pointer. Each payload
    /// field gets a fixed 8-byte slot so binding offsets need no type info.
    /// A record value (spec 0006): the enum payload layout without a tag —
    /// one 8-byte slot per field, in declaration order.
    fn emit_record_value(&mut self, fields: &[IrExpr]) -> Result<()> {
        let size = (fields.len() as u32 * 8).max(8);
        let ptr = self.fresh_local(WasmTy::I32);
        self.line(&format!("i32.const {size}"));
        let drop_idx = self.ctx.drops.of(&DropKey::Record {
            mask: fields.iter().map(|field| is_heap(&field.ty())).collect(),
        });
        self.call_alloc(drop_idx);
        self.line(&format!("local.set {ptr}"));
        for (index, field) in fields.iter().enumerate() {
            let slot = WasmTy::of(&field.ty());
            let offset = index as u32 * 8;
            self.line(&format!("local.get {ptr}"));
            self.line(&format!("i32.const {offset}"));
            self.line("i32.add");
            self.emit(field)?;
            self.line(slot.store());
        }
        self.line(&format!("local.get {ptr}"));
        Ok(())
    }

    fn emit_enum_value(&mut self, drop_idx: u32, tag: u32, payload: &[IrExpr]) -> Result<()> {
        let size = 8 + payload.len() as u32 * 8;
        let ptr = self.fresh_local(WasmTy::I32);
        self.line(&format!("i32.const {size}"));
        self.call_alloc(drop_idx);
        self.line(&format!("local.set {ptr}"));
        self.line(&format!("local.get {ptr}"));
        self.line(&format!("i32.const {tag}"));
        self.line("i32.store");
        for (index, field) in payload.iter().enumerate() {
            let slot = WasmTy::of(&field.ty());
            let offset = 8 + index as u32 * 8;
            self.line(&format!("local.get {ptr}"));
            self.line(&format!("i32.const {offset}"));
            self.line("i32.add");
            self.emit(field)?;
            self.line(slot.store());
        }
        self.line(&format!("local.get {ptr}"));
        Ok(())
    }

    /// Lowers a `match` to a tag dispatch that yields the matched arm's value.
    fn emit_match(&mut self, scrutinee: &IrExpr, arms: &[IrArm], result_ty: &Type) -> Result<()> {
        let subject = self.fresh_local(WasmTy::I32);
        self.emit(scrutinee)?;
        self.line(&format!("local.set {subject}"));
        let label = format!("$match_{}", self.counter);
        self.counter += 1;
        self.line(&format!(
            "(block {label} (result {})",
            WasmTy::of(result_ty).keyword()
        ));
        for arm in arms {
            self.emit_arm(&subject, arm, &label)?;
        }
        self.line("unreachable");
        self.line(")");
        Ok(())
    }

    fn emit_arm(&mut self, subject: &str, arm: &IrArm, done: &str) -> Result<()> {
        let mark = self.scope.len();
        match &arm.pattern {
            IrPattern::Variant { tag, bindings, .. } => {
                self.line(&format!("local.get {subject}"));
                self.line("i32.load");
                self.line(&format!("i32.const {tag}"));
                self.line("i32.eq");
                self.line("(if");
                self.line("(then");
                self.bind_payload(subject, bindings);
                self.emit_arm_body(arm, done)?;
                self.line("))");
            }
            IrPattern::Wildcard { binding } => {
                if let Some((name, ty)) = binding {
                    let local = self.fresh_local(WasmTy::of(ty));
                    self.line(&format!("local.get {subject}"));
                    self.line(&format!("local.set {local}"));
                    self.bind(name.clone(), Slot::Local(local));
                }
                self.emit_arm_body(arm, done)?;
            }
        }
        self.scope.truncate(mark);
        Ok(())
    }

    fn bind_payload(&mut self, subject: &str, bindings: &[Option<(String, Type)>]) {
        for (index, binding) in bindings.iter().enumerate() {
            if let Some((name, ty)) = binding {
                let slot = WasmTy::of(ty);
                let local = self.fresh_local(slot);
                let offset = 8 + index as u32 * 8;
                self.line(&format!("local.get {subject}"));
                self.line(&format!("i32.const {offset}"));
                self.line("i32.add");
                self.line(slot.load());
                self.line(&format!("local.set {local}"));
                self.bind(name.clone(), Slot::Local(local));
            }
        }
    }

    fn emit_arm_body(&mut self, arm: &IrArm, done: &str) -> Result<()> {
        match &arm.guard {
            Some(guard) => {
                self.emit(guard)?;
                self.line("(if");
                self.line("(then");
                self.emit(&arm.body)?;
                self.line(&format!("br {done}"));
                self.line("))");
            }
            None => {
                self.emit(&arm.body)?;
                self.line(&format!("br {done}"));
            }
        }
        Ok(())
    }

    /// Wraps the success value on the stack into a `[ok:1][value]` Result
    /// pointer (spec 0011's IR lowering note).
    fn wrap_ok(&mut self, ret: &Type) {
        let value = self.fresh_local(WasmTy::of(ret));
        self.line(&format!("local.set {value}"));
        let res = self.fresh_local(WasmTy::I32);
        self.line("i32.const 16");
        let leak = self.ctx.drops.of(&DropKey::Leak);
        self.call_alloc(leak);
        self.line(&format!("local.set {res}"));
        self.line(&format!("local.get {res}"));
        self.line("i32.const 1");
        self.line("i32.store");
        self.line(&format!("local.get {res}"));
        self.line("i32.const 8");
        self.line("i32.add");
        self.line(&format!("local.get {value}"));
        self.line(WasmTy::of(ret).store());
        self.line(&format!("local.get {res}"));
    }

    /// Raises the error value on the stack: branch to the nearest enclosing
    /// `catch`, or build and return an `Err` Result from a throwing function.
    /// The function-boundary path first releases what the frame still owns
    /// (0048 A7); an enclosing catch does its own cleanup at entry.
    fn raise_error(&mut self) {
        if let Some(label) = self.catch_stack.last() {
            let label = label.clone();
            self.line(&format!("br {label}"));
        } else {
            let err = self.fresh_local(WasmTy::I32);
            self.line(&format!("local.set {err}"));
            self.release_live_managed(0);
            let res = self.fresh_local(WasmTy::I32);
            self.line("i32.const 16");
            let leak = self.ctx.drops.of(&DropKey::Leak);
            self.call_alloc(leak);
            self.line(&format!("local.set {res}"));
            self.line(&format!("local.get {res}"));
            self.line("i32.const 0");
            self.line("i32.store");
            self.line(&format!("local.get {res}"));
            self.line("i32.const 8");
            self.line("i32.add");
            self.line(&format!("local.get {err}"));
            self.line("i32.store");
            self.line(&format!("local.get {res}"));
            self.line("return");
        }
    }

    /// Unwraps a Result pointer on the stack: on `Err`, raise the error; on
    /// `Ok`, leave the success value of type `success_ty`. The box is
    /// single-owner scratch (spec 0011's encoding, never IR-visible), so it is
    /// freed here on both paths once its payload is extracted.
    fn unwrap_result(&mut self, success_ty: &Type) -> Result<()> {
        let r = self.fresh_local(WasmTy::I32);
        self.line(&format!("local.set {r}"));
        self.line(&format!("local.get {r}"));
        self.line("i32.load");
        self.line("i32.eqz");
        self.line("(if");
        self.line("(then");
        let err = self.fresh_local(WasmTy::I32);
        self.line(&format!("local.get {r}"));
        self.line("i32.const 8");
        self.line("i32.add");
        self.line("i32.load");
        self.line(&format!("local.set {err}"));
        self.free_result_box(&r);
        self.line(&format!("local.get {err}"));
        self.raise_error();
        self.line("))");
        let value = self.fresh_local(WasmTy::of(success_ty));
        self.line(&format!("local.get {r}"));
        self.line("i32.const 8");
        self.line("i32.add");
        self.line(WasmTy::of(success_ty).load());
        self.line(&format!("local.set {value}"));
        self.free_result_box(&r);
        self.line(&format!("local.get {value}"));
        Ok(())
    }

    /// Frees a Result box: `alloc(16)` rounds to a 24-byte block with its
    /// header, and `$free` takes that total.
    fn free_result_box(&mut self, r: &str) {
        self.line(&format!("local.get {r}"));
        self.line("i32.const 24");
        self.line("call $free");
    }

    fn emit_question(&mut self, value: &IrExpr) -> Result<()> {
        // `?` applies only to throwing calls (spec 0011/0042). The inner
        // throwing call already unwraps and branches to the catch on error, so
        // `?` just yields its success value.
        self.emit(value)
    }

    fn emit_try(
        &mut self,
        body: &IrExpr,
        arms: &[IrArm],
        ty: &Type,
        err_name: Option<&str>,
    ) -> Result<()> {
        let id = self.counter;
        self.counter += 1;
        let try_label = format!("$try_{id}");
        let catch_label = format!("$catch_{id}");
        self.line(&format!(
            "(block {try_label} (result {})",
            WasmTy::of(ty).keyword()
        ));
        // The catch block yields the error pointer that a throwing call/`throw`
        // branches to it with.
        self.line(&format!("(block {catch_label} (result i32)"));
        let cleanup_mark = self.managed.len();
        self.catch_stack.push(catch_label);
        self.emit(body)?;
        self.catch_stack.pop();
        self.line(&format!("br {try_label}"));
        self.line(")");
        let err = self.fresh_local(WasmTy::I32);
        self.line(&format!("local.set {err}"));
        // Unwinding skipped the body's releases: everything the body still
        // owns is a non-zero managed local from the mark on (0048 A7).
        self.release_live_managed(cleanup_mark);
        // The caught error itself is owned; the RC pass releases it at each
        // arm's tails through this binding.
        if let Some(err_name) = err_name {
            self.managed.push((err_name.to_string(), err.clone()));
            self.bind(err_name.to_string(), Slot::Local(err.clone()));
        }
        for arm in arms {
            self.emit_arm(&err, arm, &try_label)?;
        }
        self.line("unreachable");
        self.line(")");
        Ok(())
    }

    fn emit_var(&mut self, name: &str) -> Result<()> {
        match self.slot(name) {
            Some(Slot::Local(id)) => {
                let id = id.clone();
                self.line(&format!("local.get {id}"));
                // A transfer temporary's single read consumes it: zero the
                // local so unwind cleanup cannot release it again (0048 A7).
                if self.transfer_heap.contains(name) {
                    self.line("i32.const 0");
                    self.line(&format!("local.set {id}"));
                }
            }
            Some(Slot::Capture(offset, ty)) => {
                let (offset, load) = (*offset, ty.load());
                self.line("local.get $p0");
                self.line(&format!("i32.const {offset}"));
                self.line("i32.add");
                self.line(load);
            }
            None => {
                return Err(BackendError::new(format!(
                    "unbound variable `{name}` in wasm"
                )));
            }
        }
        Ok(())
    }

    /// Allocates `[len: i32][elem...]` and returns the pointer.
    fn emit_array(&mut self, elem_ty: &Type, elems: &[IrExpr]) -> Result<()> {
        let elem = WasmTy::of(elem_ty);
        let total = 4 + elem.size() * elems.len() as u32;
        let arr = self.fresh_local(WasmTy::I32);
        self.line(&format!("i32.const {total}"));
        let drop_idx = self.ctx.drops.of(&DropTable::array_key(elem_ty));
        self.call_alloc(drop_idx);
        self.line(&format!("local.set {arr}"));
        self.line(&format!("local.get {arr}"));
        self.line(&format!("i32.const {}", elems.len()));
        self.line("i32.store");
        for (index, value) in elems.iter().enumerate() {
            let offset = 4 + elem.size() * index as u32;
            self.line(&format!("local.get {arr}"));
            self.line(&format!("i32.const {offset}"));
            self.line("i32.add");
            self.emit(value)?;
            self.line(elem.store());
        }
        self.line(&format!("local.get {arr}"));
        Ok(())
    }

    /// `array_get_unchecked(a, i)`: load the `i`-th element from `[len][elem...]`.
    /// The element address is `a + 4 + i * size`. A heap element is retained:
    /// the caller receives an owned reference (spec 0048).
    fn emit_array_get(&mut self, array: &IrExpr, index: &IrExpr, elem_ty: &Type) -> Result<()> {
        let elem = WasmTy::of(elem_ty);
        self.emit(array)?;
        self.line("i32.const 4");
        self.line("i32.add");
        self.emit(index)?;
        self.line(&format!("i32.const {}", elem.size()));
        self.line("i32.mul");
        self.line("i32.add");
        self.line(elem.load());
        if is_heap(elem_ty) {
            self.line("call $rc_retain");
        }
        Ok(())
    }

    /// `array_push(a, x)`: allocate a fresh `[len+1][elem...]`, copy `a`'s
    /// elements, then append `x`. `a` is left unchanged (pure copy).
    fn emit_array_push(&mut self, array: &IrExpr, value: &IrExpr, elem_ty: &Type) -> Result<()> {
        let elem = WasmTy::of(elem_ty);
        let size = elem.size();
        let a = self.fresh_local(WasmTy::I32);
        let len = self.fresh_local(WasmTy::I32);
        let out = self.fresh_local(WasmTy::I32);
        self.emit(array)?;
        self.line(&format!("local.set {a}"));
        self.line(&format!("local.get {a}"));
        self.line("i32.load");
        self.line(&format!("local.set {len}"));
        // out = alloc(4 + (len + 1) * size)
        self.line("i32.const 4");
        self.line(&format!("local.get {len}"));
        self.line("i32.const 1");
        self.line("i32.add");
        self.line(&format!("i32.const {size}"));
        self.line("i32.mul");
        self.line("i32.add");
        let drop_idx = self.ctx.drops.of(&DropTable::array_key(elem_ty));
        self.call_alloc(drop_idx);
        self.line(&format!("local.set {out}"));
        // store the new length (len + 1)
        self.line(&format!("local.get {out}"));
        self.line(&format!("local.get {len}"));
        self.line("i32.const 1");
        self.line("i32.add");
        self.line("i32.store");
        // memory.copy(dest = out+4, src = a+4, n = len * size)
        self.line(&format!("local.get {out}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {a}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {len}"));
        self.line(&format!("i32.const {size}"));
        self.line("i32.mul");
        self.line("memory.copy");
        // The copied elements are now shared with `a`: retain each (spec 0048).
        if is_heap(elem_ty) {
            let i = self.fresh_local(WasmTy::I32);
            let done = format!("$push_done_{}", self.counter);
            let each = format!("$push_each_{}", self.counter);
            self.counter += 1;
            self.line("i32.const 0");
            self.line(&format!("local.set {i}"));
            self.line(&format!("block {done}"));
            self.line(&format!("loop {each}"));
            self.line(&format!("local.get {i}"));
            self.line(&format!("local.get {len}"));
            self.line("i32.ge_s");
            self.line(&format!("br_if {done}"));
            self.line(&format!("local.get {out}"));
            self.line("i32.const 4");
            self.line("i32.add");
            self.line(&format!("local.get {i}"));
            self.line(&format!("i32.const {size}"));
            self.line("i32.mul");
            self.line("i32.add");
            self.line("i32.load");
            self.line("call $rc_retain");
            self.line("drop");
            self.line(&format!("local.get {i}"));
            self.line("i32.const 1");
            self.line("i32.add");
            self.line(&format!("local.set {i}"));
            self.line(&format!("br {each}"));
            self.line("end");
            self.line("end");
        }
        // store x at out + 4 + len * size
        self.line(&format!("local.get {out}"));
        self.line("i32.const 4");
        self.line("i32.add");
        self.line(&format!("local.get {len}"));
        self.line(&format!("i32.const {size}"));
        self.line("i32.mul");
        self.line("i32.add");
        self.emit(value)?;
        self.line(elem.store());
        self.line(&format!("local.get {out}"));
        Ok(())
    }

    /// A bare top-level function used as a value: a closure with no captures.
    fn emit_function_ref(&mut self, name: &str) -> Result<()> {
        let index = *self.ctx.table.toplevel.get(name).ok_or_else(|| {
            BackendError::new(format!("function `{name}` is not in the wasm table"))
        })?;
        let closure = self.fresh_local(WasmTy::I32);
        self.line("i32.const 4");
        let drop_idx = self.ctx.drops.of(&DropKey::Plain { total: 16 });
        self.call_alloc(drop_idx);
        self.line(&format!("local.set {closure}"));
        self.line(&format!("local.get {closure}"));
        self.line(&format!("i32.const {index}"));
        self.line("i32.store");
        self.line(&format!("local.get {closure}"));
        Ok(())
    }

    /// A `fn` lambda used as a value: allocate `[table_index, captures...]`.
    fn emit_closure(&mut self, node: &IrExpr, captures: &[IrCapture]) -> Result<()> {
        let index = *self
            .ctx
            .table
            .lambda_index
            .get(&(node as *const IrExpr))
            .ok_or_else(|| BackendError::new("internal error: lambda not in table"))?;
        let total = closure_size(captures);
        let closure = self.fresh_local(WasmTy::I32);
        self.line(&format!("i32.const {total}"));
        let drop_idx = self.ctx.drops.of(&DropTable::closure_key(captures));
        self.call_alloc(drop_idx);
        self.line(&format!("local.set {closure}"));
        self.line(&format!("local.get {closure}"));
        self.line(&format!("i32.const {index}"));
        self.line("i32.store");
        for ((name, offset, ty), capture) in capture_layout(captures).into_iter().zip(captures) {
            self.line(&format!("local.get {closure}"));
            self.line(&format!("i32.const {offset}"));
            self.line("i32.add");
            // The capture's value comes from the *enclosing* scope. The
            // environment owns its heap captures (spec 0048): retain on store,
            // released by the closure's drop glue.
            self.emit_var(&name)?;
            if is_heap(&capture.ty) {
                self.line("call $rc_retain");
            }
            self.line(ty.store());
        }
        self.line(&format!("local.get {closure}"));
        Ok(())
    }

    fn emit_call(&mut self, callee: &IrExpr, args: &[IrExpr]) -> Result<()> {
        if let Some(name) = self.ctx.table.is_direct(callee) {
            // Direct call to a known top-level function: env is unused (0).
            let name = name.to_string();
            self.line("i32.const 0");
            for arg in args {
                self.emit(arg)?;
            }
            self.line(&format!("call $f_{name}"));
            return Ok(());
        }

        // Indirect call through a closure value.
        let Type::Function(ft) = callee.ty() else {
            return Err(BackendError::new(
                "wasm backend cannot call a non-function value",
            ));
        };
        let sig = WasmSig::of_type(&ft);
        let sig_index = self.ctx.sigs.index_of(&sig).ok_or_else(|| {
            BackendError::new("internal error: indirect call signature was not declared")
        })?;

        self.emit(callee)?;
        let closure = self.fresh_local(WasmTy::I32);
        self.line(&format!("local.set {closure}"));
        self.line(&format!("local.get {closure}")); // environment = closure pointer
        for arg in args {
            self.emit(arg)?;
        }
        self.line(&format!("local.get {closure}"));
        self.line("i32.load"); // table index
        self.line(&format!("call_indirect (type $sig_{sig_index})"));
        Ok(())
    }
}

fn binary_op(op: BinaryOp, operand_ty: &Type) -> &'static str {
    match (WasmTy::of(operand_ty), op) {
        (WasmTy::I32, BinaryOp::Add) => "i32.add",
        (WasmTy::I32, BinaryOp::Sub) => "i32.sub",
        (WasmTy::I32, BinaryOp::Mul) => "i32.mul",
        (WasmTy::I32, BinaryOp::Div) => "i32.div_s",
        (WasmTy::I32, BinaryOp::Rem) => "i32.rem_s",
        (WasmTy::I32, BinaryOp::Eq) => "i32.eq",
        (WasmTy::I32, BinaryOp::Lt) => "i32.lt_s",
        (WasmTy::F64, BinaryOp::Add) => "f64.add",
        (WasmTy::F64, BinaryOp::Sub) => "f64.sub",
        (WasmTy::F64, BinaryOp::Mul) => "f64.mul",
        (WasmTy::F64, BinaryOp::Div) => "f64.div",
        (WasmTy::F64, BinaryOp::Eq) => "f64.eq",
        (WasmTy::F64, BinaryOp::Lt) => "f64.lt",
        // `%` is Int-only (spec 0016); the type checker rejects Float `%`.
        (WasmTy::F64, BinaryOp::Rem) => unreachable!("Float % rejected by type checker"),
        // `++` lowers to `IrExpr::Concat`, never to a Binary (spec 0017).
        (_, BinaryOp::Concat) => unreachable!("concat lowers to IrExpr::Concat"),
        // `!= > <= >=` desugar to `eq`/`lt` calls in lowering (spec 0027).
        (_, BinaryOp::Ne | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge) => {
            unreachable!("derived comparison desugared before lowering")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_codegen::{EffectRow, IrFunction, IrParam};

    fn int_fn(params: Vec<Type>) -> FunctionType {
        FunctionType {
            params,
            ret: Box::new(Type::Int),
            throws: None,
            effects: EffectRow::default(),
        }
    }

    fn main_returning(ret: Type, body: IrExpr) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                ret,
                throws: None,
                effects: EffectRow::default(),
                body,
            }],
        }
    }

    fn compile_binary(program: &IrProgram) -> Vec<u8> {
        WasmBackend
            .compile(program, &BackendOptions::default())
            .expect("compile")
            .bytes
    }

    fn compile_wat(program: &IrProgram) -> String {
        let artifact = WasmBackend
            .compile(
                program,
                &BackendOptions {
                    mode: EmitMode::Text,
                    ..Default::default()
                },
            )
            .expect("compile");
        String::from_utf8(artifact.bytes).unwrap()
    }

    // `fn add(x, y) -> Int { x + y }` and `fn main() -> Int { add(20, 22) }`.
    fn add_program() -> IrProgram {
        let add = IrFunction {
            name: "add".into(),
            params: vec![
                IrParam {
                    name: "x".into(),
                    ty: Type::Int,
                },
                IrParam {
                    name: "y".into(),
                    ty: Type::Int,
                },
            ],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::Binary {
                op: BinaryOp::Add,
                ty: Type::Int,
                left: Box::new(IrExpr::Var {
                    name: "x".into(),
                    ty: Type::Int,
                }),
                right: Box::new(IrExpr::Var {
                    name: "y".into(),
                    ty: Type::Int,
                }),
            },
        };
        let main = IrFunction {
            name: "main".into(),
            params: vec![],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef {
                    name: "add".into(),
                    sig: int_fn(vec![Type::Int, Type::Int]),
                }),
                args: vec![IrExpr::Int(20), IrExpr::Int(22)],
                ret: Type::Int,
            },
        };
        IrProgram {
            functions: vec![add, main],
        }
    }

    #[test]
    fn compiles_to_a_valid_wasm_binary() {
        let bytes = compile_binary(&add_program());
        assert_eq!(&bytes[0..4], b"\0asm");
        assert_eq!(&bytes[4..8], &[1, 0, 0, 0]);
    }

    #[test]
    fn emits_expected_wat() {
        let wat = compile_wat(&add_program());
        assert!(wat.contains("call $f_add"), "{wat}");
        assert!(wat.contains("(export \"_start\" (func $_start))"), "{wat}");
        assert!(wat.contains("call $proc_exit"), "{wat}");
    }

    #[test]
    fn emits_the_rc_runtime() {
        let wat = compile_wat(&add_program());
        assert!(
            wat.contains("(func $alloc (param $n i32) (param $drop i32) (result i32)"),
            "{wat}"
        );
        assert!(
            wat.contains("(func $free (param $p i32) (param $size i32)"),
            "{wat}"
        );
        assert!(wat.contains("(func $rc_retain (param $p i32)"), "{wat}");
        assert!(wat.contains("(func $rc_release (param $p i32)"), "{wat}");
        assert!(wat.contains("(type $drop_t (func (param i32)))"), "{wat}");
        assert!(
            wat.contains(&format!(
                "(memory (export \"memory\") {MEMORY_PAGES} {MEMORY_MAX_PAGES})"
            )),
            "{wat}"
        );
        assert!(wat.contains("(export \"live_bytes\")"), "{wat}");
        assert!(wat.contains("(export \"heap_top\")"), "{wat}");
        let _ = compile_binary(&add_program());
    }

    #[test]
    fn string_literal_becomes_a_data_segment() {
        let program = main_returning(Type::String, IrExpr::String("Hello, Emela!".into()));
        let wat = compile_wat(&program);
        assert!(wat.contains("(data (i32.const 16) \"\\0d"), "{wat}");
        let _ = compile_binary(&program);
    }

    #[test]
    fn array_allocates_and_stores_elements() {
        let program = main_returning(
            Type::Array(Box::new(Type::Int)),
            IrExpr::Array {
                elem_ty: Type::Int,
                elems: vec![IrExpr::Int(10), IrExpr::Int(20), IrExpr::Int(30)],
            },
        );
        let wat = compile_wat(&program);
        assert!(wat.contains("call $alloc"), "{wat}");
        assert!(wat.contains("i32.store"), "{wat}");
        let _ = compile_binary(&program);
    }

    #[test]
    fn emits_drop_glue_for_heap_shapes() {
        let program = main_returning(
            Type::Array(Box::new(Type::Int)),
            IrExpr::Array {
                elem_ty: Type::Int,
                elems: vec![IrExpr::Int(1)],
            },
        );
        let wat = compile_wat(&program);
        assert!(wat.contains("(func $drop_leak"), "{wat}");
        assert!(wat.contains("(func $drop_string"), "{wat}");
        assert!(wat.contains("(func $drop_array_4v"), "{wat}");
        assert!(wat.contains("call $rc_release"), "{wat}");
        let _ = compile_binary(&program);
    }

    #[test]
    fn float_array_uses_f64_stores() {
        let program = main_returning(
            Type::Array(Box::new(Type::Float)),
            IrExpr::Array {
                elem_ty: Type::Float,
                elems: vec![IrExpr::Float(1.5), IrExpr::Float(2.5)],
            },
        );
        let wat = compile_wat(&program);
        assert!(wat.contains("f64.store"), "{wat}");
        let _ = compile_binary(&program);
    }

    // `fn make_adder(n) { fn(x) { x + n } }` and a `main` that calls the result.
    fn closure_program() -> IrProgram {
        let lambda = IrExpr::Fn {
            params: vec![IrParam {
                name: "x".into(),
                ty: Type::Int,
            }],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            captures: vec![IrCapture {
                name: "n".into(),
                ty: Type::Int,
            }],
            body: Box::new(IrExpr::Binary {
                op: BinaryOp::Add,
                ty: Type::Int,
                left: Box::new(IrExpr::Var {
                    name: "x".into(),
                    ty: Type::Int,
                }),
                right: Box::new(IrExpr::Var {
                    name: "n".into(),
                    ty: Type::Int,
                }),
            }),
        };
        let make_adder = IrFunction {
            name: "make_adder".into(),
            params: vec![IrParam {
                name: "n".into(),
                ty: Type::Int,
            }],
            ret: Type::Function(int_fn(vec![Type::Int])),
            throws: None,
            effects: EffectRow::default(),
            body: lambda,
        };
        let main = IrFunction {
            name: "main".into(),
            params: vec![],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::Let {
                name: "add10".into(),
                value_ty: Type::Function(int_fn(vec![Type::Int])),
                value: Box::new(IrExpr::Call {
                    callee: Box::new(IrExpr::FunctionRef {
                        name: "make_adder".into(),
                        sig: FunctionType {
                            params: vec![Type::Int],
                            ret: Box::new(Type::Function(int_fn(vec![Type::Int]))),
                            throws: None,
                            effects: EffectRow::default(),
                        },
                    }),
                    args: vec![IrExpr::Int(10)],
                    ret: Type::Function(int_fn(vec![Type::Int])),
                }),
                next: Box::new(IrExpr::Call {
                    callee: Box::new(IrExpr::Var {
                        name: "add10".into(),
                        ty: Type::Function(int_fn(vec![Type::Int])),
                    }),
                    args: vec![IrExpr::Int(32)],
                    ret: Type::Int,
                }),
            },
        };
        IrProgram {
            functions: vec![make_adder, main],
        }
    }

    #[test]
    fn closures_use_table_and_indirect_calls() {
        let wat = compile_wat(&closure_program());
        assert!(wat.contains("(table "), "{wat}");
        assert!(wat.contains("(elem "), "{wat}");
        assert!(wat.contains("call_indirect (type $sig_"), "{wat}");
        assert!(wat.contains("$lambda_0"), "{wat}");
        // The captured `n` is loaded from the environment pointer.
        assert!(wat.contains("local.get $p0"), "{wat}");
        let _ = compile_binary(&closure_program());
    }

    /// A rewritten self-tail-call (spec 0045) compiles to a function-head
    /// `loop` and a `br` back to it — no `call` — and still validates.
    #[test]
    fn self_tail_call_emits_a_loop() {
        let spin = IrFunction {
            name: "spin".into(),
            params: vec![IrParam {
                name: "n".into(),
                ty: Type::Int,
            }],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::If {
                cond: Box::new(IrExpr::Binary {
                    op: BinaryOp::Eq,
                    ty: Type::Int,
                    left: Box::new(IrExpr::Var {
                        name: "n".into(),
                        ty: Type::Int,
                    }),
                    right: Box::new(IrExpr::Int(0)),
                }),
                then: Box::new(IrExpr::Int(42)),
                els: Box::new(IrExpr::TailSelfCall {
                    args: vec![IrExpr::Binary {
                        op: BinaryOp::Sub,
                        ty: Type::Int,
                        left: Box::new(IrExpr::Var {
                            name: "n".into(),
                            ty: Type::Int,
                        }),
                        right: Box::new(IrExpr::Int(1)),
                    }],
                    ty: Type::Int,
                }),
                ty: Type::Int,
            },
        };
        let main = IrFunction {
            name: "main".into(),
            params: vec![],
            ret: Type::Int,
            throws: None,
            effects: EffectRow::default(),
            body: IrExpr::Call {
                callee: Box::new(IrExpr::FunctionRef {
                    name: "spin".into(),
                    sig: int_fn(vec![Type::Int]),
                }),
                args: vec![IrExpr::Int(3)],
                ret: Type::Int,
            },
        };
        let program = IrProgram {
            functions: vec![spin, main],
        };
        let wat = compile_wat(&program);
        assert!(wat.contains("(loop $tail (result i32)"), "{wat}");
        assert!(wat.contains("br $tail"), "{wat}");
        // The tail call reassigns the parameter local before branching.
        assert!(wat.contains("local.set $p1"), "{wat}");
        let _ = compile_binary(&program);
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;
    use emela_codegen::{EffectRow, IrFunction};

    fn main_platform(name: &str) -> IrProgram {
        IrProgram {
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                ret: Type::Unit,
                throws: None,
                effects: EffectRow::sorted(vec!["io".into()]),
                body: IrExpr::Platform {
                    name: name.into(),
                    args: vec![IrExpr::String("hi".into())],
                    ret: Type::Unit,
                    throws: None,
                },
            }],
        }
    }

    #[test]
    fn emits_wasi_glue_and_validates() {
        let program = main_platform("io.write_stdout");
        let wat = String::from_utf8(
            WasmBackend
                .compile(
                    &program,
                    &BackendOptions {
                        mode: EmitMode::Text,
                        ..Default::default()
                    },
                )
                .expect("compile")
                .bytes,
        )
        .unwrap();
        assert!(
            wat.contains("(import \"wasi_snapshot_preview1\" \"fd_write\""),
            "{wat}"
        );
        assert!(wat.contains("(func $plat_io_write_stdout"), "{wat}");
        assert!(wat.contains("call $plat_io_write_stdout"), "{wat}");
        // Default mode assembles and validates the binary.
        let bytes = WasmBackend
            .compile(&program, &BackendOptions::default())
            .expect("compile")
            .bytes;
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn rejects_unprovided_platform_fn() {
        let err = WasmBackend
            .compile(&main_platform("fs.read"), &BackendOptions::default())
            .unwrap_err();
        assert!(err.to_string().contains("does not provide"), "{err}");
    }
}
