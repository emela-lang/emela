//! The assembly of the module and the bodies of its functions.

use super::*;

impl Builder<'_> {
    // ---- assembly ----

    pub(super) fn finish(&mut self) -> (Vec<u8>, layout::Layout) {
        let mut module = WasmModule::new();

        let mut types = TypeSection::new();
        for (params, results) in &self.types {
            types.ty().function(params.clone(), results.clone());
        }
        module.section(&types);

        if !self.imports.is_empty() {
            let mut imports = ImportSection::new();
            for (m, n, ty) in &self.imports {
                imports.import(m, n, EntityType::Function(*ty));
            }
            module.section(&imports);
        }

        let mut functions = FunctionSection::new();
        for ty in &self.defs {
            functions.function(*ty);
        }
        module.section(&functions);

        let mut table = TableSection::new();
        table.table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: self.table_funcs.len() as u64,
            maximum: Some(self.table_funcs.len() as u64),
            shared: false,
        });
        module.section(&table);

        let layout = layout::Layout::new(self.data.len() as u32);
        let heap_base = layout.heap_base;
        let pages = (heap_base as u64).div_ceil(layout::PAGE as u64).max(1);
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);

        let mut globals = GlobalSection::new();
        for i in 0..GLOBALS {
            let init = match i {
                G_HEAP => ConstExpr::i32_const(heap_base as i32),
                G_STRDESC => ConstExpr::i32_const(self.str_desc as i32),
                _ => ConstExpr::i32_const(0),
            };
            globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &init,
            );
        }
        module.section(&globals);

        let mut exports = ExportSection::new();
        exports.export("memory", ExportKind::Memory, 0);
        let entry = if self.service {
            "wasi:http/handler@0.3.0#handle"
        } else {
            "wasi:cli/run@0.3.0#run"
        };
        exports.export(
            &format!("[async-lift]{entry}"),
            ExportKind::Func,
            self.entry_index,
        );
        exports.export(
            &format!("[callback][async-lift]{entry}"),
            ExportKind::Func,
            self.callback_index,
        );
        // An imported function returning a list (`get-environment`, `get-directories`) makes the
        // host allocate the result in our linear memory through `cabi_realloc` (the canonical
        // ABI's realloc).
        if let Some(&realloc) = self.rt.get("cabi_realloc") {
            exports.export("cabi_realloc", ExportKind::Func, realloc);
        }
        module.section(&exports);

        if !self.table_funcs.is_empty() {
            let funcs = self.table_funcs.clone();
            let mut elements = ElementSection::new();
            elements.segment(wasm_encoder::ElementSegment {
                mode: wasm_encoder::ElementMode::Active {
                    table: Some(0),
                    offset: &ConstExpr::i32_const(0),
                },
                elements: Elements::Functions(std::borrow::Cow::Owned(funcs)),
            });
            module.section(&elements);
        }

        let mut code = CodeSection::new();
        for body in &self.bodies {
            code.function(body);
        }
        module.section(&code);

        let mut data = DataSection::new();
        data.active(0, &ConstExpr::i32_const(0), self.data.iter().copied());
        module.section(&data);

        (module.finish(), layout)
    }

    // ---- function bodies ----

    pub(super) fn emit_ir_bodies(&mut self) {
        let m = self.m;
        let mut bodies = Vec::new();
        for (id, f) in m.functions.iter() {
            if self.imported(id) {
                continue;
            }
            if !self.keeps_body(id, f) {
                // The index space keeps every function (calls and table entries name it), but a
                // body the entry cannot reach costs three bytes instead of its code. A const is
                // always kept, so this one is not a const and has a single body.
                bodies.push(runtime::stub());
                continue;
            }
            if f.constant {
                // A const's body is evaluated eagerly, so it never suspends (spec 5.6).
                bodies.push(self.const_wrapper(id, f));
                bodies.push(self.emit_fn(id, f));
                continue;
            }
            let body = if suspend::is_machine(f) {
                self.machine_wrapper(id, f)
            } else if f.external.is_some() {
                self.external_stub(id, f)
            } else {
                self.emit_fn(id, f)
            };
            bodies.push(body);
        }
        bodies.append(&mut self.traps);
        self.bodies = bodies;
    }

    /// The function callers of a const call: returns the remembered value or defect, and runs the
    /// body (`_init`) on the first reference only (spec 5.6).
    pub(super) fn const_wrapper(&mut self, id: FnId, f: &Function) -> WasmFunction {
        let ret = self.repr(f.ret);
        let cell = self.const_cells[&id];
        let init = self.const_inits[&id];
        let mut locals = Vec::new();
        if ret != Repr::Void {
            locals.push((1, val(ret)));
        }
        let mut func = WasmFunction::new(locals);
        const TEMP: u32 = 0;
        {
            let mut ins = func.instructions();
            // The cell holds the value: return it without calling the runtime.
            ins.i32_const(cell as i32);
            ins.i32_load(mem(CONST_STATE, 4));
            ins.i32_const(2);
            ins.i32_eq();
            ins.if_(BlockType::Empty);
            self.load_const_value(&mut ins, cell, ret);
            ins.return_();
            ins.end();
            // 1: the cell holds the defect, which `const_enter` raised.
            ins.i32_const(cell as i32);
            ins.call(self.rt("const_enter"));
            ins.if_(BlockType::Empty);
            dummy_return(&mut ins, ret);
            ins.end();
            // 0: evaluate the body once.
            ins.call(init);
            if ret != Repr::Void {
                ins.local_set(TEMP);
            }
            ins.global_get(G_DEFECT);
            ins.if_(BlockType::Empty);
            // A failing evaluation owns nothing to keep: the IR returns a dummy value.
            if ret == Repr::Ref {
                ins.local_get(TEMP);
                let drop = self.rt("drop");
                ins.call(drop);
            }
            ins.i32_const(cell as i32);
            let leave = self.rt("const_leave");
            ins.call(leave);
            dummy_return(&mut ins, ret);
            ins.end();
            // The value is now in the cell; the caller gets its own reference.
            if ret != Repr::Void {
                ins.i32_const(cell as i32);
                ins.local_get(TEMP);
            }
            match ret {
                Repr::Void => {}
                Repr::I32 | Repr::Ref => {
                    ins.i32_store(mem(CONST_VALUE, 4));
                }
                Repr::I64 => {
                    ins.i64_store(mem(CONST_VALUE, 8));
                }
                Repr::F64 => {
                    ins.f64_store(mem(CONST_VALUE, 8));
                }
            }
            if ret == Repr::Ref {
                ins.local_get(TEMP);
                let dup = self.rt("dup");
                ins.call(dup);
            }
            ins.i32_const(cell as i32);
            let leave = self.rt("const_leave");
            ins.call(leave);
            if ret != Repr::Void {
                ins.local_get(TEMP);
            }
            ins.return_();
            ins.end();
        }
        func
    }

    /// The ordinary entry of a state machine: the IR, closures and direct calls see this
    /// signature; it enters the body without a frame.
    pub(super) fn machine_wrapper(&mut self, id: FnId, f: &Function) -> WasmFunction {
        let Some(body) = self
            .sus
            .as_ref()
            .and_then(|p| p.machine(id))
            .map(|m| m.body)
        else {
            self.internal(format!("no state machine for {}", f.name));
            return WasmFunction::new(Vec::<(u32, ValType)>::new());
        };
        // The wrapper has the function's ordinary (flattened) signature and forwards every wasm
        // parameter to the body, which takes the same parameters then the frame pointer. A by-value
        // parameter is several wasm parameters, in field order.
        let params = self.signature(f).0;
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let mut ins = func.instructions();
            for i in 0..params.len() as u32 {
                ins.local_get(i);
            }
            ins.i32_const(0);
            ins.call(body);
            ins.end();
        }
        func
    }

    /// Loads a const's remembered value. A reference is duplicated: the cell keeps its own.
    pub(super) fn load_const_value(&mut self, ins: &mut InstructionSink, cell: u32, ret: Repr) {
        match ret {
            Repr::Void => {}
            Repr::I32 => {
                ins.i32_const(cell as i32);
                ins.i32_load(mem(CONST_VALUE, 4));
            }
            Repr::Ref => {
                // `dup` takes the pointer by value and counts it; load it again for the caller.
                ins.i32_const(cell as i32);
                ins.i32_load(mem(CONST_VALUE, 4));
                let dup = self.rt("dup");
                ins.call(dup);
                ins.i32_const(cell as i32);
                ins.i32_load(mem(CONST_VALUE, 4));
            }
            Repr::I64 => {
                ins.i32_const(cell as i32);
                ins.i64_load(mem(CONST_VALUE, 8));
            }
            Repr::F64 => {
                ins.i32_const(cell as i32);
                ins.f64_load(mem(CONST_VALUE, 8));
            }
        }
    }
    pub(super) fn emit_fn(&mut self, id: FnId, f: &Function) -> WasmFunction {
        let mut cx = FnCtx::new(f, self);
        let Some(body) = f.body else {
            self.internal(format!("fn {} has no body", f.name));
            return WasmFunction::new(Vec::<(u32, ValType)>::new());
        };
        let mut func = WasmFunction::new(cx.decls.clone());
        {
            let mut ins = func.instructions();
            self.seq(&mut cx, &mut ins, body, None);
            // A body that falls off its end must still return the right type.
            let leaves = cx.ret_leaves.clone();
            dummy_return_values(&mut ins, &leaves);
            ins.end();
        }
        let _ = id;
        func
    }

    pub(super) fn emit_entry(&mut self, entry: &Run) {
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let ins = &mut func.instructions();
            match entry {
                Run::Main(Some(id)) => {
                    let index = self.fn_index(*id);
                    let ret = self.repr(self.m.functions[*id].ret);
                    if self.sus.is_some() {
                        let begin = self.rt("fb_root_begin");
                        ins.call(begin);
                    }
                    ins.call(index);
                    self.drop_result(ins, ret);
                    if self.sus.is_some() {
                        let root = self.rt("fb_root_run");
                        ins.call(root);
                    }
                }
                Run::Main(None) => {}
                Run::Service(service) => {
                    // One request: the launch, then the response (`runtime/wasi/http.rs`). A
                    // launch that ended interrupted leaves no response; its unwinding is not
                    // carried to the next request.
                    if let Some(id) = service {
                        let index = self.fn_index(*id);
                        let ret = self.repr(self.m.functions[*id].ret);
                        // `Incoming` is a counted reference: the handle goes in a box.
                        ins.local_get(0);
                        ins.call(self.rt("http_box"));
                        if self.sus.is_some() {
                            let begin = self.rt("fb_root_begin");
                            ins.call(begin);
                        }
                        ins.call(index);
                        self.drop_result(ins, ret);
                        if self.sus.is_some() {
                            let root = self.rt("fb_root_run");
                            ins.call(root);
                        }
                    }
                    // The run's result is a counted value (`fb_finish_run` put it back into the
                    // global). Release it before the next request starts.
                    ins.global_get(G_DEFECT);
                    ins.call(self.rt("drop"));
                    ins.global_get(G_FAILURE);
                    ins.call(self.rt("drop"));
                    ins.i32_const(0);
                    ins.global_set(G_DEFECT);
                    ins.i32_const(0);
                    ins.global_set(G_FAILURE);
                    ins.call(self.rt("http_respond"));
                }
                Run::Tests(tests) => {
                    for (name, id) in tests {
                        // Each test is a run of its own: the previous one may have stalled.
                        if let Some(plan) = &self.sus
                            && plan.unwinding != 0
                        {
                            ins.i32_const(plan.unwinding as i32);
                            ins.i32_const(0);
                            ins.i32_store(mem(0, 4));
                        }
                        let index = self.fn_index(*id);
                        let ret = self.repr(self.m.functions[*id].ret);
                        if self.sus.is_some() {
                            let begin = self.rt("fb_root_begin");
                            ins.call(begin);
                        }
                        ins.call(index);
                        self.drop_result(ins, ret);
                        if self.sus.is_some() {
                            let root = self.rt("fb_root_run");
                            ins.call(root);
                        }
                        let name = self.string_literal(name.as_bytes());
                        ins.i32_const(name as i32);
                        ins.call(self.rt("test_result"));
                    }
                    ins.call(self.rt("test_done"));
                }
            }
            // A service keeps its output open for the next request, and `http_respond` did the
            // `task.return`.
            if !self.service {
                ins.call(self.rt("finish"));
                ins.call(self.rt("close_output"));
                ins.i32_const(0);
                ins.call(self.task_return());
            }
            // `0` is the component model's EXIT status: the async-lifted run finished
            // synchronously.
            ins.i32_const(0);
            ins.end();
        }
        self.bodies.push(func);
    }

    /// The callback of the async-lifted `run`. The generated run never yields (its waits are
    /// synchronous), so the host never calls this; it must exist for the async ABI.
    pub(super) fn emit_callback(&mut self) {
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let ins = &mut func.instructions();
            ins.i32_const(0);
            ins.end();
        }
        self.bodies.push(func);
    }

    /// Gives up a call's result: `drop` for a reference, `drop` for a plain value, nothing for
    /// `()`.
    pub(super) fn drop_result(&mut self, ins: &mut InstructionSink, ret: Repr) {
        match ret {
            Repr::Ref => {
                ins.call(self.rt("drop"));
            }
            Repr::Void => {}
            _ => {
                ins.drop();
            }
        }
    }
}
