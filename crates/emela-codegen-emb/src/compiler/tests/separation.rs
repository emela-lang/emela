use super::*;
use emela_core::{External, ExternalBinding, HostRef, Roots, Type, TypeId};
use emela_lir::{Callee, Const, FnBuilder, Module, Term, Value};
use std::{
    process::Command,
    time::{Duration, Instant},
};

fn source_flow(source: &str) -> Module {
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    crate::flow_from_core(&core).unwrap()
}

fn contents<'a>(output: &'a crate::Output, path: &str) -> &'a str {
    &output
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap()
        .contents
}

fn export(output: &crate::Output, name: &str) -> String {
    output
        .exports
        .iter()
        .find(|entry| entry.source_name == name)
        .unwrap()
        .c_symbol
        .clone()
}

// The same frozen output is consumed by the public connection API.
fn run_native(output: &crate::Output, harness: &str) {
    use crate::platform::*;
    let temp = tempfile::tempdir().unwrap();
    let tool = execution::inspect_toolchain(std::path::Path::new("cc")).unwrap();
    let before = output.files.clone();
    let mut package = native();
    package.files.push(PackageFile {
        path: "probe.c".into(),
        contents: harness.as_bytes().to_vec(),
    });
    package.sources.push("probe.c".into());
    let manifest =
        crate::artifact::Manifest::from_json(contents(output, crate::artifact::MANIFEST_FILE))
            .unwrap();
    package
        .external_symbols
        .extend(manifest.metadata.external_symbols);
    for optimization in [Optimization::O0, Optimization::O2] {
        let options = ConnectOptions {
            optimization,
            main: false,
            ..Default::default()
        };
        let plan = prepare_connection(output, &package, &options, &tool).unwrap();
        let executable = execution::execute(&plan, temp.path()).unwrap();
        let mut child = Command::new(executable).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("native entry did not finish");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        execution::verify_success(&plan, temp.path()).unwrap();
        assert_eq!(output.files, before);
    }
}
#[test]
fn native_start_runs_and_returns_for_sync_and_suspend_entries() {
    for suspends in [false, true] {
        let mut flow = Module::new(emela_core::Module::new());
        let marker = flow.add(
            FnBuilder::new(&flow.core, "mark_main").finish_external(External {
                bindings: vec![ExternalBinding {
                    target: "c".into(),
                    host: HostRef::Global("mark_main".into()),
                }],
            }),
        );
        let mut entry = FnBuilder::new(&flow.core, "main");
        entry.set_exported().set_ret(TypeId::UNIT);
        if suspends {
            entry.set_suspends();
        }
        entry.let_(
            None,
            Value::Call {
                callee: Callee::Direct(marker),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        );
        entry.term(Term::Return(None));
        flow.add(entry.finish());
        let compiled = &flow;
        let output = crate::compile_flow(compiled, &crate::Options::default()).unwrap();
        run_native(
            &output,
            r#"
#include <assert.h>
#include "emela_embedded.h"
static int calls;
void mark_main(void) { calls++; }
int main(void) { emela_embedded_start(); assert(calls == 1); return 0; }
"#,
        );
    }
}

#[test]
fn native_entry_preserves_nonvoid_suspending_result_storage() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut entry = FnBuilder::new(&flow.core, "main");
    entry.set_exported().set_suspends().set_ret(TypeId::INT);
    entry.term(Term::Return(Some(Const::Int(42).into())));
    flow.add(entry.finish());
    let compiled = &flow;
    let output = crate::compile_flow(compiled, &crate::Options::default()).unwrap();
    run_native(
        &output,
        "#include \"emela_embedded.h\"\nint main(void) { emela_embedded_start(); return 0; }\n",
    );
}

#[test]
fn native_entry_releases_managed_results_before_returning() {
    for suspends in [false, true] {
        let mut core = emela_core::Module::new();
        let list = core.types.intern(Type::List(TypeId::INT));
        let mut flow = Module::new(core);
        let mut entry = FnBuilder::new(&flow.core, "main");
        entry.set_exported().set_ret(list);
        if suspends {
            entry.set_suspends();
        }
        let result = entry.assign(
            "result",
            list,
            Value::List {
                elems: vec![Const::Int(42).into()],
                tail: None,
            },
        );
        entry.term(Term::Return(Some(result.into())));
        flow.add(entry.finish());
        let compiled = &flow;
        let output =
            crate::compile_flow(compiled, &crate::Options::default().with_heap_bytes(1024))
                .unwrap();
        run_native(
            &output,
            "#include \"emela_embedded.h\"\nint main(void) { for (int i = 0; i < 1000; ++i) emela_embedded_start(); return 0; }\n",
        );
    }
}

#[test]
fn string_plans_preserve_aliases_while_other_buffers_and_calls_are_live() {
    let flow = source_flow(
        r#"
fn number(x: Int) -> String { Int.to_string(x) }
pub fn render(x: Int) -> String {
  padded = String.append(" ", number(x))
  same = String.append(padded, "")
  trimmed = String.trim(same)
  next = number(x + 1)
  String.append(String.append(trimmed, ":"), next)
}
"#,
    );
    let compiled = &flow;
    let options = crate::Options::default().with_string_capacity(64);
    let output = crate::compile_flow(compiled, &options).unwrap();
    let render = export(&output, "render");
    run_native(
        &output,
        &format!(
            r#"
#include <assert.h>
#include <string.h>
#include "emela_embedded.h"
int main(void) {{
  const int32_t inputs[] = {{0, 41, -1, INT32_MIN, INT32_MAX}};
  const char *expected[] = {{"0:1", "41:42", "-1:0", "-2147483648:-2147483647", "2147483647:-2147483648"}};
  for (int round = 0; round < 1000; ++round) {{
    for (unsigned i = 0; i < sizeof inputs / sizeof inputs[0]; ++i) {{
      char buffer[64];
      EmelaString value = {render}(inputs[i], buffer, UINT16_C(64));
      assert(value.length == strlen(expected[i]));
      assert(memcmp(value.data, expected[i], value.length) == 0);
      assert(value.data == buffer && value.owner == NULL);
    }}
  }}
  return 0;
}}
"#
        ),
    );
}

#[test]
fn shared_string_helpers_support_multiple_consumers_in_one_translation_unit() {
    let flow = source_flow(
        r#"
pub fn has_a(text: String) -> Bool { String.contains(text, "a") }
pub fn starts_a(text: String) -> Bool { String.starts_with(text, "a") }
pub fn ends_a(text: String) -> Bool { String.ends_with(text, "a") }
"#,
    );
    let compiled = &flow;
    let options = crate::Options::default();
    let output = crate::compile_flow(compiled, &options).unwrap();
    let has = export(&output, "has_a");
    let starts = export(&output, "starts_a");
    let ends = export(&output, "ends_a");
    run_native(
        &output,
        &format!(
            r#"
#include <assert.h>
#include "emela_embedded.h"
int main(void) {{
  EmelaString both = {{.data = "aba", .length = 3}};
  EmelaString middle = {{.data = "bab", .length = 3}};
  EmelaString empty = {{.data = "", .length = 0}};
  assert({has}(both) && {starts}(both) && {ends}(both));
  assert({has}(middle) && !{starts}(middle) && !{ends}(middle));
  assert(!{has}(empty) && !{starts}(empty) && !{ends}(empty));
  return 0;
}}
"#
        ),
    );
}

#[test]
fn common_closure_lowering_is_shared_and_native_managed_values_run() {
    let flow =
        source_flow("pub fn calculate(x: Int) -> Int { add = fn(y: Int) { x + y }\n add(2) }");
    let output =
        crate::compile_flow(&flow, &crate::Options::default().with_heap_bytes(2048)).unwrap();
    let calculate = export(&output, "calculate");
    run_native(
        &output,
        &format!(
            "#include <assert.h>\n#include \"emela_embedded.h\"\nint main(void) {{for(int i=0;i<1000;++i) assert({calculate}(40)==42);return 0;}}\n"
        ),
    );
}
#[test]
fn common_entries_supply_dynamic_string_output_buffers() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut reader = FnBuilder::new(&flow.core, "read_number");
    reader.set_ret(TypeId::INT);
    let reader = flow.add(reader.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("read_number".into()),
        }],
    }));
    let mut entry = FnBuilder::new(&flow.core, "main");
    entry.set_exported().set_ret(TypeId::STRING);
    let number = entry.assign(
        "number",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(reader),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    let text = entry.assign(
        "text",
        TypeId::STRING,
        Value::Builtin {
            op: emela_core::Builtin::IntToString,
            args: vec![number.into()],
        },
    );
    entry.term(Term::Return(Some(text.into())));
    flow.add(entry.finish());
    for capacity in [2, 64, 512] {
        let output = crate::compile_flow(
            &flow,
            &crate::Options::default().with_string_capacity(capacity),
        )
        .unwrap();
        let name = export(&output, "main");
        run_native(
            &output,
            &format!(
                "#include <assert.h>\n#include <string.h>\n#include \"emela_embedded.h\"\nstatic int calls;\nint32_t read_number(void) {{calls++;return 42;}}\nint main(void) {{for(int i=0;i<1000;++i) emela_embedded_start();assert(calls==1000);char buffer[2];EmelaString value={name}(buffer,2);assert(value.data==buffer && value.owner==NULL && value.length==2 && memcmp(buffer,\"42\",2)==0);return 0;}}\n"
            ),
        );
    }
}

#[test]
fn shared_planners_use_feature_facts_instead_of_concrete_operations() {
    use syn::visit::{self, Visit};
    struct Operations;
    impl<'ast> Visit<'ast> for Operations {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            for segment in &path.segments {
                assert!(
                    !["Builtin", "PrimOp", "RuntimeOp", "FiberOp"]
                        .contains(&segment.ident.to_string().as_str()),
                    "shared planner inspects concrete operation {}",
                    segment.ident
                );
            }
            visit::visit_path(self, path);
        }
    }
    for source in [
        include_str!("../plan/strings.rs"),
        include_str!("../plan/heap.rs"),
        include_str!("../plan/runtime.rs"),
    ] {
        Operations.visit_file(&syn::parse_file(source).unwrap());
    }
}
