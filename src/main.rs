mod ast;
mod codegen;
mod driver;
mod error;
mod external;
mod lexer;
mod parser;
mod platform;
mod typecheck;

fn main() {
    if let Err(error) = driver::run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{FunctionType, PrimType, Type};
    use crate::codegen::{
        emit_assembly, emit_assembly_for_platform, emit_js, emit_js_library, native_link_args,
    };
    use crate::driver::{
        compile_source, compile_source_for_platform, compile_source_for_platform_with_mode,
        compile_source_for_target,
    };
    use crate::external::ExternalRegistry;
    use crate::platform::PlatformSpec;
    use crate::platform::Target;
    use crate::typecheck::CheckMode;

    #[test]
    fn accepts_empty_main() {
        let (_, typed) = compile_source("fn main() -> Unit {\n}\n").unwrap();
        assert_eq!(typed.functions[0].name, "main");
        assert_eq!(typed.functions[0].ret, Type::Prim(PrimType::Unit));
    }

    #[test]
    fn infers_i32_function() {
        let (_, typed) = compile_source(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}

fn main() -> I32 {
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
        assert_eq!(
            add.params,
            vec![Type::Prim(PrimType::I32), Type::Prim(PrimType::I32)]
        );
        assert_eq!(add.ret, Type::Prim(PrimType::I32));
    }

    #[test]
    fn accepts_return_annotation_and_exits_with_main_i32() {
        let source = r#"
fn add(x: i32, y: i32) -> i32 {
  x + y
}

fn main() -> I32 {
  add(20, 22)
}
"#;
        let (program, typed) =
            compile_source_for_target(source, Target::Aarch64AppleDarwin).unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert_eq!(main.ret, Type::Prim(PrimType::I32));

        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains(".globl _main"));
    }

    #[test]
    fn library_mode_allows_sources_without_main() {
        let source = r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}
"#;
        compile_source(source).unwrap_err();
        let platform = PlatformSpec::native_for_target(Target::Aarch64AppleDarwin);
        let (_, typed) =
            compile_source_for_platform_with_mode(source, &platform, CheckMode::Library).unwrap();
        assert_eq!(typed.functions.len(), 1);
        assert_eq!(typed.functions[0].name, "add");
    }

    #[test]
    fn supports_parameter_and_local_type_annotations() {
        let (_, typed) = compile_source(
            r#"
fn add(x: I32, y: I32) -> I32 {
  sum: I32 = x + y
  sum
}

fn main() -> I32 {
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
        assert_eq!(
            add.params,
            vec![Type::Prim(PrimType::I32), Type::Prim(PrimType::I32)]
        );
        assert_eq!(add.ret, Type::Prim(PrimType::I32));
    }

    #[test]
    fn rejects_mismatched_parameter_type_annotation() {
        let error = compile_source(
            r#"
fn negate(value: Bool) -> Bool {
  value == 0
}

fn main() -> Bool {
  negate(true)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn rejects_mismatched_local_type_annotation() {
        let error = compile_source(
            r#"
fn main() -> I32 {
  value: Bool = 42
  0
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn rejects_missing_parameter_type_annotation() {
        let error = compile_source(
            r#"
fn add(x, y: I32) -> I32 {
  x + y
}

fn main() -> I32 {
  add(1, 2)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("must have a type annotation"));
    }

    #[test]
    fn rejects_missing_return_type_annotation() {
        let error = compile_source(
            r#"
fn main() {
  ()
}
"#,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("must have a return type annotation"));
    }

    #[test]
    fn rejects_missing_local_type_annotation() {
        let error = compile_source(
            r#"
fn main() -> I32 {
  value = 42
  value
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("must have a type annotation"));
    }

    #[test]
    fn supports_function_values_in_bindings_and_parameters() {
        let (_, typed) = compile_source(
            r#"
fn add_one(value: I32) -> I32 {
  value + 1
}

fn apply(value: I32, f: fn(I32) -> I32) -> I32 {
  f(value)
}

fn main() -> I32 {
  op: fn(I32) -> I32 = add_one
  apply(41, op)
}
"#,
        )
        .unwrap();
        let apply = typed
            .functions
            .iter()
            .find(|function| function.name == "apply")
            .unwrap();
        assert_eq!(
            apply.params[1],
            Type::Function(FunctionType {
                params: vec![Type::Prim(PrimType::I32)],
                ret: Box::new(Type::Prim(PrimType::I32)),
                effectful: false,
            })
        );
    }

    #[test]
    fn supports_functions_returning_functions() {
        let (_, typed) = compile_source(
            r#"
fn double(value: I32) -> I32 {
  value * 2
}

fn identity(value: I32) -> I32 {
  value
}

fn choose_transform(flag: Bool) -> fn(I32) -> I32 {
  match flag {
    true -> double
    false -> identity
  }
}

fn main() -> I32 {
  transform: fn(I32) -> I32 = choose_transform(true)
  transform(21)
}
"#,
        )
        .unwrap();
        let choose_transform = typed
            .functions
            .iter()
            .find(|function| function.name == "choose_transform")
            .unwrap();
        assert_eq!(
            choose_transform.ret,
            Type::Function(FunctionType {
                params: vec![Type::Prim(PrimType::I32)],
                ret: Box::new(Type::Prim(PrimType::I32)),
                effectful: false,
            })
        );
    }

    #[test]
    fn rejects_function_value_type_mismatch() {
        let error = compile_source(
            r#"
fn is_zero(value: I32) -> Bool {
  value == 0
}

fn main() -> I32 {
  op: fn(I32) -> I32 = is_zero
  op(41)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn rejects_effectful_function_value_call_from_pure_function() {
        let error = compile_source(
            r#"
fn tick!() -> Unit {
  ()
}

fn call(callback: fn!() -> Unit) -> Unit {
  callback()
}

fn main() -> Unit {
  call(tick!)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("pure function"));
    }

    #[test]
    fn native_backend_rejects_function_values() {
        let (program, typed) = compile_source(
            r#"
fn add_one(value: I32) -> I32 {
  value + 1
}

fn main() -> I32 {
  op: fn(I32) -> I32 = add_one
  op(41)
}
"#,
        )
        .unwrap();
        let error = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap_err();
        assert!(error
            .to_string()
            .contains("does not support function value"));
    }

    #[test]
    fn emits_assembly_match_expression() {
        let (program, typed) = compile_source_for_target(
            "fn main() -> I32 { match true { true -> 1 false -> 0 } }",
            Target::Aarch64AppleDarwin,
        )
        .unwrap();
        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("mov w10, #1"));
        assert!(assembly.contains("mov w10, #0"));
        assert!(assembly.contains("cmp w9, w10"));
    }

    #[test]
    fn emits_x86_64_linux_assembly() {
        let (program, typed) = compile_source_for_target(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}

fn main() -> I32 {
  add(20, 22)
}
"#,
            Target::X86_64UnknownLinuxGnu,
        )
        .unwrap();
        let assembly = emit_assembly(Target::X86_64UnknownLinuxGnu, &program, &typed).unwrap();
        assert!(assembly.contains(".globl main"));
        assert!(assembly.contains("add:"));
        assert!(assembly.contains("movq %rdi, -8(%rbp)"));
        assert!(assembly.contains("call add"));
        assert!(assembly.contains("addl %r9d, %eax"));
    }

    #[test]
    fn rejects_match_pattern_type_mismatch() {
        let error =
            compile_source("fn main() -> I32 { match 1 { true -> 2 false -> 3 } }").unwrap_err();
        assert!(error.to_string().contains("type mismatch"));
    }

    #[test]
    fn rejects_effectful_call_from_pure_function() {
        let error = compile_source(
            r#"
fn tick!() -> Unit {
  ()
}

fn main() -> Unit {
  tick!()
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("pure function"));
    }

    #[test]
    fn allows_main_effect_boundary_with_capability() {
        let (program, typed) = compile_source(
            r#"
#[requires(Stdout)]
fn print_i32!(value: I32) -> Unit {
  ()
}

fn main!() -> I32 {
  print_i32!(42)
  0
}
"#,
        )
        .unwrap();
        let main = typed
            .functions
            .iter()
            .find(|function| function.name == "main!")
            .unwrap();
        assert!(main.effectful);
        assert_eq!(main.capabilities.len(), 1);

        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains(".globl _main"));
        assert!(assembly.contains("requires Stdout"));
    }

    #[test]
    fn rejects_requires_outside_declared_capabilities() {
        let error = compile_source(
            r#"
#[requires(Stdout)]
fn print_i32!(value: I32) -> Unit {
  ()
}

#[requires()]
fn main!() -> Unit {
  print_i32!(42)
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside #[requires"));
    }

    #[test]
    fn lowers_primitive_method_calls() {
        let (program, typed) = compile_source(
            r#"
fn main() -> I32 {
  20.add(22)
}
"#,
        )
        .unwrap();
        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("add w0, w9, w0"));
    }

    #[test]
    fn supports_struct_enum_and_result_pattern_matching() {
        let (program, typed) = compile_source(
            r#"
struct Error {
  code: I32
}

enum Result {
  Ok(I32)
  Err(Error)
}

fn checked(value: I32) -> Result {
  match value == 0 {
    true -> Err(Error { code: 7 })
    false -> Ok(value)
  }
}

fn main() -> I32 {
  match checked(0) {
    Ok(value) -> value
    Err(error) -> error.code
  }
}
"#,
        )
        .unwrap();
        let checked = typed
            .functions
            .iter()
            .find(|function| function.name == "checked")
            .unwrap();
        assert_eq!(checked.ret, Type::Named("Result".to_string()));

        let assembly = emit_assembly(Target::Aarch64AppleDarwin, &program, &typed).unwrap();
        assert!(assembly.contains("orr x0, x0, #0"));
        assert!(assembly.contains("orr x0, x0, #1"));
        assert!(assembly.contains("lsr x9, x9, #32"));
    }

    #[test]
    fn rejects_capability_missing_from_native_target() {
        let error = compile_source(
            r#"
#[requires(HostImport)]
fn host_call!() -> Unit {
  ()
}

fn main!() -> Unit {
  host_call!()
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not provide"));
    }

    #[test]
    fn target_capability_set_is_checked() {
        let source = r#"
#[requires(Stdout)]
fn print_i32!(value: I32) -> Unit {
  ()
}

fn main!() -> Unit {
  print_i32!(42)
}
"#;
        compile_source_for_target(source, Target::Wasm32Wasi).unwrap();
        let error = compile_source_for_target(source, Target::Wasm32UnknownUnknown).unwrap_err();
        assert!(error
            .to_string()
            .contains("platform `wasm32-unknown-unknown` does not provide"));
    }

    #[test]
    fn imported_external_capability_set_is_checked() {
        let source = r#"
import platform.io.print_i32!

fn main!() -> Unit {
  print_i32!(42)
}
"#;
        compile_source_for_target(source, Target::Wasm32Wasi).unwrap();
        let error = compile_source_for_target(source, Target::Wasm32UnknownUnknown).unwrap_err();
        assert!(error
            .to_string()
            .contains("platform `wasm32-unknown-unknown` does not provide"));
    }

    #[test]
    fn rejects_imported_effectful_call_from_pure_function() {
        let error = compile_source_for_target(
            r#"
import platform.io.print_i32!

fn main() -> Unit {
  print_i32!(42)
}
"#,
            Target::Aarch64AppleDarwin,
        )
        .unwrap_err();
        assert!(error.to_string().contains("pure function"));
    }

    #[test]
    fn imported_external_function_lowers_to_native_binding() {
        let platform = platform_from_manifest(NATIVE_MANIFEST);
        let (program, typed) = compile_source_for_platform(
            r#"
import platform.io.print_i32!

fn main!() -> Unit {
  print_i32!(42)
}
"#,
            &platform,
        )
        .unwrap();
        let darwin =
            emit_assembly_for_platform(Target::Aarch64AppleDarwin, &platform, &program, &typed)
                .unwrap();
        assert!(darwin.contains("    mov w0, #42\n"));
        assert!(darwin.contains("    str x0, [sp, #0]\n"));
        assert!(darwin.contains("    ldr x0, [sp, #0]\n"));
        assert!(darwin.contains("    bl _emela_print_i32\n"));

        let linux =
            emit_assembly_for_platform(Target::X86_64UnknownLinuxGnu, &platform, &program, &typed)
                .unwrap();
        assert!(linux.contains("    movl $42, %eax\n"));
        assert!(linux.contains("    movq %rax, 0(%rsp)\n"));
        assert!(linux.contains("    movq 0(%rsp), %rdi\n"));
        assert!(linux.contains("    call emela_print_i32\n"));
    }

    #[test]
    fn native_codegen_rejects_missing_binding() {
        let platform = platform_from_manifest(MISSING_JS_BINDING_MANIFEST);
        let (program, typed) = compile_source_for_platform(
            r#"
import platform.io.print_i32!

fn main!() -> Unit {
  print_i32!(42)
}
"#,
            &platform,
        )
        .unwrap();
        let error =
            emit_assembly_for_platform(Target::Aarch64AppleDarwin, &platform, &program, &typed)
                .unwrap_err();
        assert!(error.to_string().contains("does not have a native binding"));
    }

    #[test]
    fn non_native_target_does_not_emit_assembly() {
        let (program, typed) =
            compile_source_for_target("fn main() -> Unit {}", Target::Wasm32UnknownUnknown)
                .unwrap();
        let error = emit_assembly(Target::Wasm32UnknownUnknown, &program, &typed).unwrap_err();
        assert!(error
            .to_string()
            .contains("does not have a native assembly backend"));
    }

    #[test]
    fn parses_platform_manifest_external_registry() {
        let (name, capabilities, registry) =
            ExternalRegistry::from_manifest_json(NODE_MANIFEST).unwrap();
        assert_eq!(name, "node");
        assert_eq!(capabilities, vec![crate::ast::Capability::Stdout]);
        let function = registry
            .resolve_import(&["platform".to_string(), "io".to_string()], "print_i32!")
            .unwrap();
        assert_eq!(function.params, vec![Type::Prim(PrimType::I32)]);
        assert_eq!(function.ret, Type::Prim(PrimType::Unit));
        assert_eq!(function.bindings.js_callee.as_deref(), Some("console.log"));
    }

    #[test]
    fn parses_platform_manifest_native_binding() {
        let (_, _, registry) = ExternalRegistry::from_manifest_json(NATIVE_MANIFEST).unwrap();
        let function = registry
            .resolve_import(&["platform".to_string(), "io".to_string()], "print_i32!")
            .unwrap();
        let native = function.bindings.native.as_ref().unwrap();
        assert_eq!(native.symbol, "emela_print_i32");
        assert_eq!(native.links, vec!["emela_runtime"]);
        assert_eq!(registry.native_links(), vec!["emela_runtime"]);
        let platform = platform_from_manifest(NATIVE_MANIFEST);
        let (program, _) = compile_source_for_platform(
            r#"
import platform.io.print_i32!

fn main!() -> Unit {
  print_i32!(42)
}
"#,
            &platform,
        )
        .unwrap();
        assert_eq!(
            native_link_args(&platform, &program),
            vec!["-lemela_runtime"]
        );
    }

    #[test]
    fn rejects_invalid_manifest_native_binding() {
        let error = ExternalRegistry::from_manifest_json(
            r#"
{
  "name": "bad",
  "capabilities": ["Stdout"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print_i32!",
      "params": ["I32"],
      "return": "Unit",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {
        "native": {
          "link": ["emela_runtime"]
        }
      }
    }
  ]
}
"#,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("native binding missing `symbol`"));
    }

    #[test]
    fn rejects_manifest_unknown_type() {
        let error = ExternalRegistry::from_manifest_json(
            r#"
{
  "name": "bad",
  "capabilities": [],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print!",
      "params": ["String"],
      "return": "Unit",
      "effectful": true,
      "capabilities": [],
      "bindings": {}
    }
  ]
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown external manifest type"));
    }

    #[test]
    fn rejects_manifest_duplicate_import() {
        let error = ExternalRegistry::from_manifest_json(
            r#"
{
  "name": "bad",
  "capabilities": ["Stdout"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print_i32!",
      "params": ["I32"],
      "return": "Unit",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {}
    },
    {
      "path": ["platform", "io"],
      "name": "print_i32!",
      "params": ["I32"],
      "return": "Unit",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {}
    }
  ]
}
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicate external import"));
    }

    #[test]
    fn manifest_platform_capability_set_is_checked() {
        let source = r#"
import platform.io.print_i32!

fn main!() -> Unit {
  print_i32!(42)
}
"#;
        let node = platform_from_manifest(NODE_MANIFEST);
        compile_source_for_platform(source, &node).unwrap();

        let no_stdout = platform_from_manifest(NO_STDOUT_MANIFEST);
        let error = compile_source_for_platform(source, &no_stdout).unwrap_err();
        assert!(error
            .to_string()
            .contains("platform `no-stdout` does not provide"));
    }

    #[test]
    fn emits_js_main_and_external_binding() {
        let platform = platform_from_manifest(NODE_MANIFEST);
        let (program, typed) = compile_source_for_platform(
            r#"
import platform.io.print_i32!

fn main!() -> Unit {
  print_i32!(42)
}
"#,
            &platform,
        )
        .unwrap();
        let js = emit_js(&platform, &program, &typed).unwrap();
        assert!(js.contains("function main_effect()"));
        assert!(js.contains("console.log(42);"));
        assert!(js.contains("const __emela_result = main_effect();"));
    }

    #[test]
    fn emits_js_library_without_entrypoint() {
        let platform = PlatformSpec::native_for_target(Target::Aarch64AppleDarwin);
        let (program, typed) = compile_source_for_platform_with_mode(
            r#"
fn add(x: I32, y: I32) -> I32 {
  x + y
}
"#,
            &platform,
            CheckMode::Library,
        )
        .unwrap();
        let js = emit_js_library(&platform, &program, &typed).unwrap();
        assert!(js.contains("function add(x, y)"));
        assert!(!js.contains("__emela_result"));
    }

    #[test]
    fn js_codegen_rejects_missing_binding() {
        let platform = platform_from_manifest(MISSING_JS_BINDING_MANIFEST);
        let (program, typed) = compile_source_for_platform(
            r#"
import platform.io.print_i32!

fn main!() -> Unit {
  print_i32!(42)
}
"#,
            &platform,
        )
        .unwrap();
        let error = emit_js(&platform, &program, &typed).unwrap_err();
        assert!(error.to_string().contains("does not have a js binding"));
    }

    fn platform_from_manifest(source: &str) -> PlatformSpec {
        let (name, capabilities, externs) = ExternalRegistry::from_manifest_json(source).unwrap();
        PlatformSpec {
            name,
            provided_capabilities: capabilities.into_iter().collect(),
            externs,
        }
    }

    const NODE_MANIFEST: &str = r#"
{
  "name": "node",
  "capabilities": ["Stdout"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print_i32!",
      "params": ["I32"],
      "return": "Unit",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {
        "js": {
          "callee": "console.log"
        }
      }
    }
  ]
}
"#;

    const NO_STDOUT_MANIFEST: &str = r#"
{
  "name": "no-stdout",
  "capabilities": [],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print_i32!",
      "params": ["I32"],
      "return": "Unit",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {
        "js": {
          "callee": "console.log"
        }
      }
    }
  ]
}
"#;

    const NATIVE_MANIFEST: &str = r#"
{
  "name": "native-runtime",
  "capabilities": ["Stdout"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print_i32!",
      "params": ["I32"],
      "return": "Unit",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {
        "native": {
          "symbol": "emela_print_i32",
          "link": ["emela_runtime"]
        }
      }
    }
  ]
}
"#;

    const MISSING_JS_BINDING_MANIFEST: &str = r#"
{
  "name": "missing-js",
  "capabilities": ["Stdout"],
  "externs": [
    {
      "path": ["platform", "io"],
      "name": "print_i32!",
      "params": ["I32"],
      "return": "Unit",
      "effectful": true,
      "capabilities": ["Stdout"],
      "bindings": {}
    }
  ]
}
"#;
}
