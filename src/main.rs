mod ast;
mod codegen;
mod driver;
mod error;
mod lexer;
mod parser;
mod typecheck;

fn main() {
    if let Err(error) = driver::run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::PrimType;
    use crate::codegen::emit_rust;
    use crate::driver::compile_source;

    #[test]
    fn accepts_empty_main() {
        let (_, typed) = compile_source("fn main() {\n}\n").unwrap();
        assert_eq!(typed.functions[0].name, "main");
        assert_eq!(typed.functions[0].ret, PrimType::Unit);
    }

    #[test]
    fn infers_i32_function() {
        let (_, typed) = compile_source(
            r#"
fn add(x, y) {
  x + y
}

fn main() {
  add(20, 22)
}
"#,
        )
        .unwrap();
        let add = typed
            .functions
            .iter()
            .find(|function| function.name == "add")
            .unwrap();
        assert_eq!(add.params, vec![PrimType::I32, PrimType::I32]);
        assert_eq!(add.ret, PrimType::I32);
    }

    #[test]
    fn accepts_return_annotation_and_exits_with_main_i32() {
        let source = r#"
fn add(x, y) -> i32 {
  x + y
}

fn main() {
  add(20, 22)
}
"#;
        let (program, typed) = compile_source(source).unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, PrimType::I32);

        let rust_source = emit_rust(&program, &typed);
        assert!(rust_source.contains("std::process::exit(_emela_result);"));
    }

    #[test]
    fn emits_rust_match_expression() {
        let (program, typed) =
            compile_source("fn main() -> I32 { match true { true -> 1 false -> 0 } }").unwrap();
        let rust_source = emit_rust(&program, &typed);
        assert!(rust_source.contains("match true { true => 1i32, false => 0i32, }"));
    }

    #[test]
    fn rejects_match_pattern_type_mismatch() {
        let error = compile_source("fn main() { match 1 { true -> 2 false -> 3 } }").unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }
}
