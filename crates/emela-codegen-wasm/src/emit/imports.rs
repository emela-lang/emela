//! The WASI 0.3 imports of the module, each added when first used.

use super::*;

impl Builder<'_> {
    // ---- WASI 0.3 imports ----

    pub(crate) fn environment_get(&mut self) -> u32 {
        self.host_import(
            "wasi:cli/environment@0.3.0",
            "get-environment",
            // The result is a `list<tuple<string, string>>`, returned through a pointer.
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn arguments_get(&mut self) -> u32 {
        self.host_import(
            "wasi:cli/environment@0.3.0",
            "get-arguments",
            // The result is a `list<string>`, returned through a pointer.
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn random_u64(&mut self) -> u32 {
        self.host_import(
            "wasi:random/random@0.3.0",
            "get-random-u64",
            Vec::new(),
            vec![ValType::I64],
        )
    }

    pub(crate) fn system_clock_now(&mut self) -> u32 {
        self.host_import(
            "wasi:clocks/system-clock@0.3.0",
            "now",
            // The `instant` record is returned through a pointer.
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn monotonic_wait_for(&mut self) -> u32 {
        self.host_import(
            "wasi:clocks/monotonic-clock@0.3.0",
            "[async-lower]wait-for",
            vec![ValType::I64],
            vec![ValType::I32],
        )
    }

    pub(crate) fn subtask_drop(&mut self) -> u32 {
        self.host_import("$root", "[subtask-drop]", vec![ValType::I32], Vec::new())
    }

    pub(crate) fn exit_with_code(&mut self) -> u32 {
        self.host_import(
            "wasi:cli/exit@0.3.0",
            "exit-with-code",
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn task_return(&mut self) -> u32 {
        self.host_import(
            "[export]wasi:cli/run@0.3.0",
            "[task-return]run",
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn waitable_set_new(&mut self) -> u32 {
        self.host_import(
            "$root",
            "[waitable-set-new]",
            Vec::new(),
            vec![ValType::I32],
        )
    }

    pub(crate) fn waitable_set_wait(&mut self) -> u32 {
        self.host_import(
            "$root",
            "[waitable-set-wait]",
            vec![ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    pub(crate) fn waitable_set_drop(&mut self) -> u32 {
        self.host_import(
            "$root",
            "[waitable-set-drop]",
            vec![ValType::I32],
            Vec::new(),
        )
    }

    pub(crate) fn waitable_join(&mut self) -> u32 {
        self.host_import(
            "$root",
            "[waitable-join]",
            vec![ValType::I32, ValType::I32],
            Vec::new(),
        )
    }

    /// `write-via-stream` and the stream/future intrinsics it introduces, for one of the two
    /// output interfaces (`wasi:cli/stdout@0.3.0` or `wasi:cli/stderr@0.3.0`).
    pub(crate) fn output_imports(&mut self, interface: &'static str) -> OutputImports {
        OutputImports {
            write: self.host_import(
                interface,
                "write-via-stream",
                vec![ValType::I32],
                vec![ValType::I32],
            ),
            stream_new: self.host_import(
                interface,
                "[stream-new-0]write-via-stream",
                Vec::new(),
                vec![ValType::I64],
            ),
            stream_write: self.host_import(
                interface,
                "[async-lower][stream-write-0]write-via-stream",
                vec![ValType::I32, ValType::I32, ValType::I32],
                vec![ValType::I32],
            ),
            stream_drop_writable: self.host_import(
                interface,
                "[stream-drop-writable-0]write-via-stream",
                vec![ValType::I32],
                Vec::new(),
            ),
            future_read: self.host_import(
                interface,
                "[async-lower][future-read-1]write-via-stream",
                vec![ValType::I32, ValType::I32],
                vec![ValType::I32],
            ),
            future_drop_readable: self.host_import(
                interface,
                "[future-drop-readable-1]write-via-stream",
                vec![ValType::I32],
                Vec::new(),
            ),
        }
    }
}
