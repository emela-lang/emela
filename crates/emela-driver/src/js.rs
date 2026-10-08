//! Core IR を emela-codegen-js で JS にする出力段．

use emela_codegen_js::{Options, RUNTIME, RUNTIME_FILE, RuntimeMode, emit_module};

use crate::diagnostic::Diagnostic;
use crate::output::{JsOutput, OutputFile};
use crate::pipeline::{Analysis, JsBackend};

/// エントリのモジュールの出力ファイル名．
pub const JS_ENTRY_FILE: &str = "main.mjs";

/// Core IR の1モジュールを JS にする．ランタイムは隣のファイルに書き出して import させる．
///
/// 自己末尾呼び出しのループ化はここでかける．モジュールが1つの間だけの形で，
/// lowering が複数のモジュールを出すようになったら `Program` を変える．
#[derive(Debug, Default, Clone, Copy)]
pub struct CoreJs;

impl JsBackend<emela_core::Module> for CoreJs {
    fn emit(
        &mut self,
        program: &emela_core::Module,
        _: &Analysis,
    ) -> Result<JsOutput, Vec<Diagnostic>> {
        let mut module = program.clone();
        emela_core::tail::loopify(&mut module);
        let options = Options {
            runtime: RuntimeMode::Import(None),
        };
        Ok(JsOutput {
            files: vec![
                OutputFile::new(JS_ENTRY_FILE, emit_module(&module, &options)),
                OutputFile::new(RUNTIME_FILE, RUNTIME),
            ],
            entry: JS_ENTRY_FILE.into(),
        })
    }
}
