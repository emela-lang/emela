//! `Std.Tcp` on WASI 0.3: the registry of `SystemTcp`.
//!
//! The init and the release of `SystemTcp` are reached by every `with Process` (`[ext.decl]`), so
//! their host functions are implemented here before the operations are. The operations have no
//! `wasm` binding yet, so a registry never holds a connection: `tcp_open_registry` gives a heap
//! object with one plain slot (left 0, for the state of the registry), and
//! `tcp_release_registry` has nothing to close. The object is freed by reference counting, like
//! any other value.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("tcp_open_registry", &[], &[ValType::I32]),
    ("tcp_release_registry", &[ValType::I32], &[]),
];

pub(super) fn locals(_name: &str) -> Vec<(u32, ValType)> {
    Vec::new()
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_std_host(StdHost::is_tcp)
}

pub(super) fn emit(b: &mut Builder<'_>, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "tcp_open_registry" => {
            let desc = b.desc(DescKey {
                kind: 0,
                tag: 0,
                enum_id: -1,
                name: "tcp_registry".to_owned(),
                show: None,
                refs: vec![false],
            });
            let alloc = b.rt("alloc");
            ins.i32_const(1);
            ins.i32_const(desc as i32);
            ins.call(alloc);
        }
        "tcp_release_registry" => {}
        _ => return false,
    }
    true
}
