//! The WASM runtime, emitted into the same module as the program.
//!
//! It holds the Perceus allocator (`alloc`, `dup`, `drop` with the iterative free list), the
//! string and integer conversions, the evidence tables and the WASI 0.3 host calls
//! (`write-via-stream` for `dbg` and the test output, `exit-with-code`, plus the Console, Env,
//! Random and Clock.now wrappers). It is written with
//! `wasm-encoder` like the rest of the module, so the backend has no external toolchain.
//!
//! Memory layout (see `crate::emit::layout`): a 16-byte object header (rc, aux, descriptor,
//! padding) then 8-byte slots. Static data (descriptors, string literals, nullary constructors,
//! capture-less closures) has `rc == 0` and is never counted.
//!
//! The runtime is split by feature. Each module owns its entries of `FUNCS`, its locals, its
//! static data and its lowering; `MODULES` is the whole registration list, in definition order.

use super::layout::{CPBUF, DIGITS, DIGITS64, FUTUREBUF, HOSTBUF, JSONBUF, NEWLINE, PAGE, WAITBUF};
use super::*;
use wasm_encoder::{InstructionSink, ValType};

/// A runtime function: its name and wasm signature.
pub(super) type Sig = (&'static str, &'static [ValType], &'static [ValType]);

/// One runtime module: its functions, the locals of each, its static data and its lowering.
pub(super) struct RuntimeModule {
    funcs: &'static [Sig],
    locals: fn(&str) -> Vec<(u32, ValType)>,
    /// Writes the module's static data before any runtime body is emitted, and records the
    /// offsets in `Builder::rt_data` for the lowering to read.
    setup: fn(&mut Builder),
    emit: fn(&mut Builder, &str, &mut InstructionSink) -> bool,
    /// Whether the module is part of the output at all. The Float runtime is heavy (the exact
    /// decimal of `to_string`), so it is only added when the program uses a Float builtin; a
    /// module without Float keeps the same bytes.
    used: fn(&Builder) -> bool,
}

fn always(_: &Builder) -> bool {
    true
}

mod alloc;
pub(super) mod async_bridge;
pub(super) mod attempt;
mod canon;
mod clock;
mod console;
mod constant;
mod defect;
mod env;
mod float;
pub(super) mod fs;
mod host;
mod notes;
mod numeric;
mod option;
mod protocol;
mod random;
mod sched;
mod show;
mod string;
mod unicode;
mod unicode_tables;

/// Every runtime module, in definition order.
///
/// A new feature (Float, the file system, scheduling, ...) adds its own module with `FUNCS`,
/// `locals`, `setup` and `emit`, and appends one entry here. The order of the existing
/// entries must not change, so function indices and the emitted bytes stay stable.
const MODULES: &[RuntimeModule] = &[
    RuntimeModule {
        funcs: alloc::FUNCS,
        locals: alloc::locals,
        setup: no_setup,
        emit: alloc::emit,
        used: always,
    },
    RuntimeModule {
        funcs: string::FUNCS,
        locals: string::locals,
        setup: no_setup,
        emit: string::emit,
        used: always,
    },
    RuntimeModule {
        funcs: option::FUNCS,
        locals: option::locals,
        setup: no_setup,
        emit: option::emit,
        used: always,
    },
    RuntimeModule {
        funcs: show::FUNCS,
        locals: show::locals,
        setup: no_setup,
        emit: show::emit,
        used: always,
    },
    RuntimeModule {
        funcs: host::FUNCS,
        locals: host::locals,
        setup: no_setup,
        emit: host::emit,
        used: always,
    },
    RuntimeModule {
        funcs: defect::FUNCS,
        locals: defect::locals,
        setup: no_setup,
        emit: defect::emit,
        used: always,
    },
    RuntimeModule {
        funcs: protocol::FUNCS,
        locals: protocol::locals,
        setup: no_setup,
        emit: protocol::emit,
        used: always,
    },
    RuntimeModule {
        funcs: numeric::FUNCS,
        locals: numeric::locals,
        setup: no_setup,
        emit: numeric::emit,
        used: always,
    },
    RuntimeModule {
        funcs: float::FUNCS,
        locals: float::locals,
        setup: no_setup,
        emit: float::emit,
        used: float::used,
    },
    RuntimeModule {
        funcs: unicode::FUNCS,
        locals: unicode::locals,
        setup: unicode::setup,
        emit: unicode::emit,
        used: always,
    },
    RuntimeModule {
        funcs: constant::FUNCS,
        locals: constant::locals,
        setup: no_setup,
        emit: constant::emit,
        used: always,
    },
    RuntimeModule {
        funcs: notes::FUNCS,
        locals: notes::locals,
        setup: no_setup,
        emit: notes::emit,
        used: always,
    },
    RuntimeModule {
        funcs: console::FUNCS,
        locals: console::locals,
        setup: no_setup,
        emit: console::emit,
        used: console::used,
    },
    RuntimeModule {
        funcs: env::FUNCS,
        locals: env::locals,
        setup: no_setup,
        emit: env::emit,
        used: env::used,
    },
    RuntimeModule {
        funcs: canon::FUNCS,
        locals: canon::locals,
        setup: no_setup,
        emit: canon::emit,
        used: canon::used,
    },
    RuntimeModule {
        funcs: random::FUNCS,
        locals: random::locals,
        setup: no_setup,
        emit: random::emit,
        used: random::used,
    },
    RuntimeModule {
        funcs: clock::FUNCS,
        locals: clock::locals,
        setup: no_setup,
        emit: clock::emit,
        used: clock::used,
    },
    RuntimeModule {
        funcs: sched::FUNCS,
        locals: sched::locals,
        // The scheduler's cells are allocated by `emit::suspend::prepare`.
        setup: no_setup,
        emit: sched::emit,
        used: super::suspend::used,
    },
    RuntimeModule {
        funcs: async_bridge::FUNCS,
        locals: async_bridge::locals,
        setup: no_setup,
        emit: async_bridge::emit,
        used: async_bridge::used,
    },
    RuntimeModule {
        funcs: fs::FUNCS,
        locals: fs::locals,
        // The plan's cells are allocated by `fs::prepare`, before the index space is assigned.
        setup: no_setup,
        emit: fs::emit,
        used: fs::used,
    },
    RuntimeModule {
        funcs: attempt::FUNCS,
        locals: attempt::locals,
        setup: no_setup,
        emit: attempt::emit,
        used: attempt::used,
    },
];

/// Reserves the runtime functions in the function index space.
pub(crate) fn register(b: &mut Builder) {
    if float::used(b) {
        float::prepare(b);
    }
    async_bridge::prepare(b);
    fs::prepare(b);
    for module in MODULES {
        if !(module.used)(b) {
            continue;
        }
        for (name, params, results) in module.funcs {
            let ty = b.ty(params.to_vec(), results.to_vec());
            let index = b.imports.len() as u32 + b.defs.len() as u32;
            b.defs.push(ty);
            b.rt.insert(name, index);
        }
    }
}

/// The `setup` of a module without static data.
fn no_setup(_: &mut Builder) {}

/// `n` i32 locals. Their indices start after the parameters; each `locals` arm names them in a
/// comment, in index order.
pub(super) fn i32s(n: u32) -> Vec<(u32, ValType)> {
    vec![(n, ValType::I32)]
}

fn rt(b: &Builder, name: &'static str) -> u32 {
    b.rt[name]
}

/// Writes the static data of every module, then emits every runtime function body, in `MODULES`
/// order.
pub(crate) fn emit(b: &mut Builder) {
    for module in MODULES {
        (module.setup)(b);
    }
    for module in MODULES {
        if !(module.used)(b) {
            continue;
        }
        for (name, ..) in module.funcs {
            let mut f = WasmFunction::new((module.locals)(name));
            {
                let mut ins = f.instructions();
                let handled = (module.emit)(b, name, &mut ins);
                if !handled {
                    b.internal(format!("no lowering for runtime function `{name}`"));
                }
                ins.end();
            }
            b.bodies.push(f);
        }
    }
}
