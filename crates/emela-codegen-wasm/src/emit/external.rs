//! External functions crossing the WASM boundary as host imports.

use super::*;

use emela_core::{HostModule, HostRef};

impl<'m> Builder<'m> {
    pub(super) fn external_import(
        &mut self,
        id: FnId,
        f: &Function,
        binding: &emela_core::ExternalBinding,
    ) {
        let (module, name) = match &binding.host {
            HostRef::Import { module, name } => {
                let module = match module {
                    HostModule::Named(n) => n.clone(),
                    HostModule::File(p) | HostModule::Bundled(p) => p.clone(),
                };
                (module, name.clone())
            }
            HostRef::Global(_) => {
                self.internal(format!(
                    "`@external(wasm, ...)` of `{}` is not a module/name import",
                    f.name
                ));
                return;
            }
        };
        let mut params = Vec::new();
        for &p in &f.params {
            match self.host_val(f.vars[p].ty) {
                Some(v) => params.push(v),
                None => {
                    self.host_type_error(f, f.vars[p].ty);
                    return;
                }
            }
        }
        let results: Vec<ValType> = match self.host_val(f.ret) {
            Some(v) => vec![v],
            None => vec![],
        };
        let ty = self.ty(params, results);
        if let Some(&i) = self.import_map.get(&(module.clone(), name.clone())) {
            self.fn_index[ix(id)] = i;
            return;
        }
        let index = self.imports.len() as u32;
        self.imports.push((module.clone(), name.clone(), ty));
        self.import_map.insert((module, name), index);
        self.fn_index[ix(id)] = index;
    }

    fn host_val(&self, ty: TypeId) -> Option<ValType> {
        Some(match self.core().ty(ty) {
            Type::Int | Type::Bool => ValType::I32,
            Type::Int64 => ValType::I64,
            Type::Float => ValType::F64,
            _ => return None,
        })
    }

    fn host_type_error(&mut self, f: &Function, ty: TypeId) {
        let name = emela_lir::core_ty(self.core(), ty);
        let kind = if matches!(self.core().ty(ty), Type::Fn(_)) {
            WasmErrorKind::Callback
        } else {
            WasmErrorKind::Unsupported
        };
        let message = if kind == WasmErrorKind::Callback {
            format!(
                "external function `{}` takes a callback, which target `wasm` does not support yet",
                f.name
            )
        } else {
            format!(
                "external function `{}` uses the type {name}, which cannot cross to WASM",
                f.name
            )
        };
        self.errors.push(WasmError::new(kind, message));
    }

    pub(super) fn external_stub(&mut self, _id: FnId, f: &Function) -> WasmFunction {
        let msg = format!(
            "external function `{}` is not available on this target",
            check::display_name(&f.name)
        );
        let s = self.string_literal(msg.as_bytes());
        let ret = self.repr(f.ret);
        let mut func = WasmFunction::new(Vec::<(u32, ValType)>::new());
        {
            let mut ins = func.instructions();
            ins.i32_const(s as i32);
            let raise = self.rt("raise");
            ins.call(raise);
            dummy_return(&mut ins, ret);
            ins.end();
        }
        func
    }
}
