//! コンパイラに同梱する Prelude と core のソース（仕様 15.2）．
//!
//! ソースは `lib/` の `.emel` で，`include_str!` で埋め込む．Emela で書けない関数は
//! `@intrinsic` を付けた本体のない fn として宣言し，実装は `Builtin::lookup_intrinsic` で
//! 組み込み関数の表から引く．

/// Prelude のソースのモジュール名．`prelude.emel` はソースのルートに置けない予約名（4.1）．
pub const PRELUDE: &str = "Prelude";

/// 同梱のソース．モジュール名とソースの組で，Prelude が先頭．
const SOURCES: &[(&str, &str)] = &[
    (PRELUDE, include_str!("../lib/prelude.emel")),
    ("List", include_str!("../lib/list.emel")),
    ("Option", include_str!("../lib/option.emel")),
    ("String", include_str!("../lib/string.emel")),
    ("Int", include_str!("../lib/int.emel")),
    ("Int64", include_str!("../lib/int64.emel")),
    ("Float", include_str!("../lib/float.emel")),
    ("Duration", include_str!("../lib/duration.emel")),
    ("Instant", include_str!("../lib/instant.emel")),
    ("Console", include_str!("../lib/console.emel")),
    ("JsConsole", include_str!("../lib/js_console.emel")),
];

/// 同梱の JS のファイル．`lib/` からの相対パスと中身の組．core の `@external(js, "./x.mjs", …)`
/// が指すファイルで，`.emel` と同じく `include_str!` で埋め込む（手順は `lib/README.md`）．
const JS_FILES: &[(&str, &str)] = &[];

/// 同梱の JS のファイルを書き出す，JS の出力先の中のディレクトリ．core の `@external` の
/// ファイルの指定は，lowering がエントリの出力（出力先の直下）からの相対パス
/// （`./emela_core/x.mjs`）に直す．
pub const CORE_JS_DIR: &str = "emela_core";

/// 同梱する Prelude と core のソースを，モジュール名とソースの組で返す．Prelude が先頭．
pub fn core_sources() -> &'static [(&'static str, &'static str)] {
    SOURCES
}

/// 同梱の JS のファイルを，`lib/` からの相対パスと中身の組で返す．
pub fn core_js_files() -> &'static [(&'static str, &'static str)] {
    JS_FILES
}

/// モジュール名で同梱のソースを引く．
pub fn core_source(module: &str) -> Option<&'static str> {
    SOURCES.iter().find(|(m, _)| *m == module).map(|(_, s)| *s)
}

#[cfg(test)]
mod tests {
    use emela_syntax::ast::{AstNode, FnDecl, HasAttrs};
    use emela_syntax::{DiagnosticCode, SyntaxKind, SyntaxNode, parse};

    use super::*;
    use crate::{Builtin, Sig};

    fn fn_decls(src: &str) -> Vec<FnDecl> {
        parse(src).tree().fn_decls().collect()
    }

    fn name(f: &FnDecl) -> String {
        f.name().expect("関数名がある").text().to_owned()
    }

    /// 空白を除いた型の書き方．
    fn text(node: &SyntaxNode) -> String {
        node.text().to_string().split_whitespace().collect()
    }

    /// 組み込み関数の表の型を Emela の書き方にする．型引数は `A`，`B`，…．
    fn render(sig: Sig) -> String {
        match sig {
            Sig::Int => "Int".into(),
            Sig::Int64 => "Int64".into(),
            Sig::Float => "Float".into(),
            Sig::Bool => "Bool".into(),
            Sig::String => "String".into(),
            Sig::Unit => "()".into(),
            Sig::Never => "Never".into(),
            Sig::List(t) => format!("List[{}]", render(*t)),
            Sig::Option(t) => format!("Option[{}]", render(*t)),
            Sig::Param(i) => char::from(b'A' + i as u8).to_string(),
        }
    }

    #[test]
    fn sources_parse_without_syntax_errors() {
        for (module, src) in core_sources() {
            let parse = parse(src);
            let intrinsics: Vec<_> = fn_decls(src)
                .into_iter()
                .filter(|f| f.has_annotation("intrinsic"))
                .map(|f| f.syntax().text_range())
                .collect();
            // 今の検査は本体のない fn に `@external` を求める（E0130）．`@intrinsic` の宣言に出る
            // E0130 だけは検査の側で外す前提で許し，それ以外の診断はすべて誤りとする．
            let errors: Vec<_> = parse
                .diagnostics()
                .iter()
                .filter(|d| {
                    !(d.code == DiagnosticCode::MissingExternal
                        && intrinsics.iter().any(|r| r.contains_range(d.range)))
                })
                .collect();
            assert!(errors.is_empty(), "{module} の構文エラー: {errors:#?}");
            let errors = parse
                .syntax()
                .descendants()
                .filter(|n| n.kind() == SyntaxKind::ERROR)
                .count();
            assert_eq!(errors, 0, "{module} に ERROR のノードがある");
        }
    }

    #[test]
    fn intrinsics_match_the_table() {
        let mut declared = Vec::new();
        for (module, src) in core_sources() {
            for f in fn_decls(src) {
                let name = name(&f);
                if !f.has_annotation("intrinsic") {
                    assert!(
                        f.body().is_some() || f.has_annotation("external"),
                        "{module}.{name} に本体がない"
                    );
                    continue;
                }
                assert!(
                    f.body().is_none(),
                    "@intrinsic の {module}.{name} に本体がある"
                );
                assert!(f.is_pub(), "@intrinsic の {module}.{name} が pub でない");
                let op = Builtin::lookup_intrinsic(module, &name)
                    .unwrap_or_else(|| panic!("{module}.{name} が組み込み関数の表にない"));
                let info = op.info();
                let params: Vec<_> = f
                    .param_list()
                    .into_iter()
                    .flat_map(|l| l.params().collect::<Vec<_>>())
                    .map(|p| text(&p.ty().expect("引数の型がある")))
                    .collect();
                let want: Vec<_> = info.params.iter().map(|s| render(*s)).collect();
                assert_eq!(params, want, "{module}.{name} の引数の型");
                let ret = f.ret_type().map(|t| text(&t));
                assert_eq!(ret, Some(render(info.ret)), "{module}.{name} の戻り値の型");
                let ty_params = f
                    .type_params()
                    .map_or(0, |list| list.children().count() as u32);
                assert_eq!(ty_params, info.ty_params, "{module}.{name} の型引数の数");
                declared.push(op);
            }
        }
        for op in Builtin::ALL {
            let n = declared.iter().filter(|d| *d == op).count();
            assert_eq!(n, 1, "{} の @intrinsic の宣言が {n} 個", op.info().path());
        }
    }

    /// `lib/` の `.mjs` は全部 `JS_FILES` に載せる（載せ忘れるとインストールした emela から読めない）．
    #[test]
    fn js_files_match_lib_dir() {
        let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lib");
        let mut on_disk = Vec::new();
        let mut dirs = vec![lib.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "mjs") {
                    let rel = path.strip_prefix(&lib).unwrap();
                    on_disk.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        on_disk.sort();
        let mut listed: Vec<String> = core_js_files().iter().map(|(p, _)| p.to_string()).collect();
        listed.sort();
        assert_eq!(listed, on_disk, "lib/ の .mjs と JS_FILES が食い違う");
    }

    #[test]
    fn console_is_bundled() {
        let modules: Vec<_> = core_sources().iter().map(|(m, _)| *m).collect();
        assert_eq!(modules[modules.len() - 2..], ["Console", "JsConsole"]);
        assert!(core_source("Option").unwrap().contains("pub fn or_fail"));
        assert!(
            core_source("JsConsole")
                .unwrap()
                .contains("pub handler JsConsole")
        );
    }

    #[test]
    fn lookup_intrinsic_maps_prelude_to_unqualified() {
        assert_eq!(
            Builtin::lookup_intrinsic(PRELUDE, "panic"),
            Some(Builtin::Panic)
        );
        assert_eq!(
            Builtin::lookup_intrinsic("String", "length"),
            Some(Builtin::StringLength)
        );
        assert_eq!(Builtin::lookup_intrinsic("String", "panic"), None);
        assert_eq!(Builtin::lookup_intrinsic(PRELUDE, "length"), None);
        assert_eq!(core_source("List"), Some(include_str!("../lib/list.emel")));
        assert_eq!(core_source("Map"), None);
    }
}
