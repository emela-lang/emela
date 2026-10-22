//! The WASM runtime, emitted into the same module as the program.
//!
//! It holds the Perceus allocator (`alloc`, `dup`, `drop` with the iterative free list), the
//! string and integer conversions, the evidence tables and the WASI host calls (`fd_write` for
//! `dbg` and the test output, `proc_exit`). It is written with `wasm-encoder` like the rest of
//! the module, so the backend has no external toolchain.
//!
//! Memory layout (see `crate::emit::layout`): a 16-byte object header (rc, aux, descriptor,
//! padding) then 8-byte slots. Static data (descriptors, string literals, nullary constructors,
//! capture-less closures) has `rc == 0` and is never counted.
//!
//! The runtime is split by feature. Each module owns its entries of `FUNCS`, its locals and its
//! lowering; `MODULES` is the whole registration list, in definition order.

use super::layout::{CPBUF, DIGITS, DIGITS64, IOVEC, JSONBUF, NEWLINE, NWRITTEN, PAGE};
use super::*;
use wasm_encoder::{InstructionSink, ValType};

/// A runtime function: its name and wasm signature.
pub(super) type Sig = (&'static str, &'static [ValType], &'static [ValType]);

/// One runtime module: its functions, the locals of each, and its lowering.
pub(super) struct RuntimeModule {
    funcs: &'static [Sig],
    locals: fn(&str) -> Vec<(u32, ValType)>,
    emit: fn(&mut Builder, &str, &mut InstructionSink) -> bool,
}

mod alloc;
mod defect;
mod host;
mod numeric;
mod option;
mod protocol;
mod show;
mod string;

/// Every runtime module, in definition order.
///
/// A new feature (Float, the file system, scheduling, Unicode tables, ...) adds its own module
/// with `FUNCS`, `locals` and `emit`, and appends one entry here. The order of the existing
/// entries must not change, so function indices and the emitted bytes stay stable.
const MODULES: &[RuntimeModule] = &[
    RuntimeModule {
        funcs: alloc::FUNCS,
        locals: alloc::locals,
        emit: alloc::emit,
    },
    RuntimeModule {
        funcs: string::FUNCS,
        locals: string::locals,
        emit: string::emit,
    },
    RuntimeModule {
        funcs: option::FUNCS,
        locals: option::locals,
        emit: option::emit,
    },
    RuntimeModule {
        funcs: show::FUNCS,
        locals: show::locals,
        emit: show::emit,
    },
    RuntimeModule {
        funcs: host::FUNCS,
        locals: host::locals,
        emit: host::emit,
    },
    RuntimeModule {
        funcs: defect::FUNCS,
        locals: defect::locals,
        emit: defect::emit,
    },
    RuntimeModule {
        funcs: protocol::FUNCS,
        locals: protocol::locals,
        emit: protocol::emit,
    },
    RuntimeModule {
        funcs: numeric::FUNCS,
        locals: numeric::locals,
        emit: numeric::emit,
    },
];

/// Reserves the runtime functions in the function index space.
pub(crate) fn register(b: &mut Builder) {
    for module in MODULES {
        for (name, params, results) in module.funcs {
            let ty = b.ty(params.to_vec(), results.to_vec());
            let index = b.imports.len() as u32 + b.defs.len() as u32;
            b.defs.push(ty);
            b.rt.insert(name, index);
        }
    }
}

/// `n` i32 locals. Their indices start after the parameters; each `locals` arm names them in a
/// comment, in index order.
pub(super) fn i32s(n: u32) -> Vec<(u32, ValType)> {
    vec![(n, ValType::I32)]
}

fn rt(b: &Builder, name: &'static str) -> u32 {
    b.rt[name]
}

/// Emits every runtime function body, in `MODULES` order.
pub(crate) fn emit(b: &mut Builder) {
    for module in MODULES {
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
