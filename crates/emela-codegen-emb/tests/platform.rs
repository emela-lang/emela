use emela_codegen_emb::{Options, Output, compile_flow, platform::*};
use emela_core::{Type, TypeId};
use emela_lir::{Const, FnBuilder, Module, Term, Value};
use std::{fs, path::Path, process::Command};
fn main_output(suspends: bool) -> Output {
    let mut flow = Module::new(emela_core::Module::new());
    let mut main = FnBuilder::new(&flow.core, "main");
    main.set_exported().set_ret(TypeId::INT);
    if suspends {
        main.set_suspends();
    }
    main.term(Term::Return(Some(Const::Int(42).into())));
    flow.add(main.finish());
    compile_flow(
        &flow,
        &Options::default().with_export_alias("board_main_start", "main"),
    )
    .unwrap()
}
fn tool() -> ToolchainInfo {
    execution::inspect_toolchain(Path::new("cc")).unwrap()
}
fn run(plan: &BuildPlan) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let executable = execution::execute(plan, temp.path()).unwrap();
    assert!(Command::new(executable).status().unwrap().success());
    execution::verify_success(plan, temp.path()).unwrap();
    temp
}
#[test]
fn frozen_output_builds_on_native_and_independent_third_platform_and_plans_both_mcus() {
    let output = main_output(true);
    let snapshot = output.files.clone();
    let exports = output.exports.clone();
    let tool = tool();
    let native_plan =
        prepare_connection(&output, &native(), &ConnectOptions::default(), &tool).unwrap();
    run(&native_plan);
    // This entry finishes in the first scheduler run, so an idle call is always premature.
    // Independent package: only generated public headers and its own services. No native code.
    let package=PlatformPackage{id:"third".into(),abi_version:2,services:["runtime.lifecycle","runtime.defect"].map(str::to_owned).into(),external_symbols:Default::default(),isr_safe_symbols:Default::default(),supports_isr:false,files:vec![PackageFile{path:"third.c".into(),contents:b"#include <stdlib.h>\n#include \"emela_board.h\"\nstatic int invoked;\nvoid emela_board_idle(void) { abort(); }\nvoid emela_platform_start(void (*run)(void)) { if(invoked++) abort();run(); }\n_Noreturn void emela_board_defect(EmelaDefectKind kind,const char *text,uint16_t length) {(void)kind;(void)text;(void)length;abort();}\n".to_vec()}],sources:vec!["third.c".into()],compile_flags:vec![],link_flags:vec!["-lm".into()]};
    for optimization in [Optimization::O0, Optimization::O2] {
        let plan = prepare_connection(
            &output,
            &package,
            &ConnectOptions {
                optimization,
                ..Default::default()
            },
            &tool,
        )
        .unwrap();
        assert_eq!(plan.common_hash, native_plan.common_hash);
        run(&plan);
    }
    for id in ["stm32f446re", "esp32c3"] {
        let package = board(id).unwrap();
        let plan = prepare_connection(
            &output,
            &package,
            &ConnectOptions {
                main: false,
                executable: "firmware.elf".into(),
                ..Default::default()
            },
            &tool,
        )
        .unwrap();
        assert_eq!(plan.common_hash, native_plan.common_hash);
        assert!(
            plan.commands[0]
                .arguments
                .iter()
                .any(|arg| arg.contains("startup.S"))
        );
    }
    assert_eq!(output.files, snapshot);
    assert_eq!(output.exports, exports);
}
#[test]
fn saved_artifact_builds_without_original_source_or_compiler_checkout() {
    let output = main_output(false);
    let artifact = tempfile::tempdir().unwrap();
    for file in &output.files {
        fs::write(artifact.path().join(&file.path), &file.contents).unwrap();
    }
    let restored = execution::load_artifact(artifact.path()).unwrap();
    assert_eq!(restored.files, output.files);
    let plan =
        prepare_connection(&restored, &native(), &ConnectOptions::default(), &tool()).unwrap();
    drop(output);
    drop(restored);
    drop(artifact);
    run(&plan);
}
#[test]
fn connection_rejects_missing_services_versions_isr_and_toolchain_inputs() {
    let output = main_output(false);
    let mut package = native();
    let tool = tool();
    let options = ConnectOptions::default();
    package.services.remove("runtime.lifecycle");
    assert!(
        prepare_connection(&output, &package, &options, &tool)
            .unwrap_err()
            .contains("runtime.lifecycle")
    );
    package = native();
    package.abi_version = 1;
    assert!(
        prepare_connection(&output, &package, &options, &tool)
            .unwrap_err()
            .contains("ABI")
    );
    let mut output = output.clone();
    let file = output
        .files
        .iter_mut()
        .find(|file| file.path == emela_codegen_emb::artifact::MANIFEST_FILE)
        .unwrap();
    let mut manifest = emela_codegen_emb::artifact::Manifest::from_json(&file.contents).unwrap();
    manifest.metadata.isr_entries.push("main".into());
    file.contents = manifest.to_json().unwrap();
    assert!(
        prepare_connection(&output, &native(), &options, &tool)
            .unwrap_err()
            .contains("ISR")
    );
}
#[test]
fn build_state_detects_changed_inputs_image_and_package() {
    let output = main_output(false);
    let plan = prepare_connection(&output, &native(), &ConnectOptions::default(), &tool()).unwrap();
    let temp = run(&plan);
    let source = temp.path().join("platform/native.c");
    fs::write(&source, "changed").unwrap();
    assert!(execution::verify_success(&plan, temp.path()).is_err());
    fs::write(
        source,
        plan.files
            .iter()
            .find(|file| file.path == "platform/native.c")
            .unwrap()
            .contents
            .clone(),
    )
    .unwrap();
    fs::write(temp.path().join(&plan.executable), "changed").unwrap();
    assert!(execution::verify_success(&plan, temp.path()).is_err());
    let mut changed = plan.clone();
    changed.platform_hash.push('x');
    assert!(execution::verify_success(&changed, temp.path()).is_err());
}
#[test]
fn generation_is_repeatable_and_does_not_mutate_its_input() {
    let mut core = emela_core::Module::new();
    let list = core.types.intern(Type::List(TypeId::INT));
    let mut flow = Module::new(core);
    let mut entry = FnBuilder::new(&flow.core, "main");
    entry.set_exported().set_ret(list);
    let value = entry.assign(
        "list",
        list,
        Value::List {
            elems: vec![Const::Int(42).into()],
            tail: None,
        },
    );
    entry.term(Term::Return(Some(value.into())));
    flow.add(entry.finish());
    let before = format!("{flow:?}");
    let options = Options::default().with_heap_bytes(1024);
    let expected = compile_flow(&flow, &options).unwrap();
    for _ in 0..8 {
        let actual = compile_flow(&flow, &options).unwrap();
        assert_eq!(actual.files, expected.files);
        assert_eq!(actual.exports, expected.exports);
    }
    assert_eq!(format!("{flow:?}"), before);
}
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;
#[test]
fn native_clock_console_and_timer_use_common_standard_service_glue() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Clock.{Clock}
import Std.Console.{Console}
import Std.Embedded.Board.{BoardClock, BoardConsole}
pub fn main() -> Int use Async {
  with BoardClock, BoardConsole {
    (use Clock).sleep(2)
    (use Console).print_line("connected")
    42
  }
}
"#,
        )],
        emela_core::Roots::Public,
    )
    .unwrap();
    let output = emela_codegen_emb::compile(
        &core,
        &Options::default()
            .with_heap_bytes(4096)
            .with_frame_bytes(2048),
    )
    .unwrap();
    let plan = prepare_connection(&output, &native(), &ConnectOptions::default(), &tool()).unwrap();
    run(&plan);
}

#[test]
fn native_timer_slots_survive_cancellation_and_repeated_reuse() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Clock.{Clock}
import Std.Fiber.{Fiber}
import Std.Embedded.Board.{BoardClock}
pub fn wait_ms() -> Int use Async {
  with BoardClock {
    Fiber.scope(fn() {
      child = Fiber.fork(fn() { (use Clock).sleep(10000) })
      (use Clock).sleep(0)
      Fiber.interrupt(child)
      42
    })
  }
}
"#,
        )],
        emela_core::Roots::Public,
    )
    .unwrap();
    let output = emela_codegen_emb::compile(
        &core,
        &Options::default()
            .with_heap_bytes(4096)
            .with_frame_bytes(2048),
    )
    .unwrap();
    let entry = &output
        .exports
        .iter()
        .find(|entry| entry.source_name == "wait_ms")
        .unwrap()
        .c_symbol;
    let mut package = native();
    let source = format!(
        r#"
#include <assert.h>
#include "emela_embedded.h"
void emela_board_idle(void);
int main(void) {{
  for (int round=0;round<100;++round) {{
    int32_t result=-1;EmelaFiber fiber={entry}(&result);
    while (emela_scheduler_run(64)) {{}}
    assert(!emela_scheduler_ready());
    emela_board_idle();
    assert(emela_scheduler_ready() && result==-1);
    for (int i=0;i<1000 && !emela_fiber_done(fiber);++i) {{emela_scheduler_run(64);emela_board_idle();}}
    assert(emela_fiber_done(fiber) && result==42);emela_fiber_release(fiber);
  }}
  return 0;
}}
"#
    );
    package.files.push(PackageFile {
        path: "probe.c".into(),
        contents: source.into_bytes(),
    });
    package.sources.push("probe.c".into());
    for optimization in [Optimization::O0, Optimization::O2] {
        let plan = prepare_connection(
            &output,
            &package,
            &ConnectOptions {
                optimization,
                main: false,
                ..Default::default()
            },
            &tool(),
        )
        .unwrap();
        run(&plan);
    }
}
