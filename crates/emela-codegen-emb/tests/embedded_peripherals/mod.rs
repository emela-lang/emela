use super::*;

mod clock;
mod diagnostics;

fn source_output(source: &str) -> Output {
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported)
        .expect("lower a source using the standard embedded handlers");
    compile(
        &core,
        &Options::default()
            .with_heap_bytes(8192)
            .with_string_capacity(96)
            .with_peripheral_buffer_capacity(4)
            .with_fiber_capacity(4)
            .with_frame_capacity(12)
            .with_frame_bytes(2048)
            .with_task_capacity(2)
            .with_event_capacity(2)
            .with_deferred_capacity(1),
    )
    .expect("emit the standard peripheral bridge")
}

fn export_name(output: &Output, stem: &str) -> String {
    let prefix = format!("emela_export_{stem}_");
    let header = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.h")
        .expect("generated public header")
        .contents;
    let declaration = header
        .lines()
        .find(|line| line.contains(&prefix))
        .expect("exported fixture function");
    declaration[declaration.find(&prefix).unwrap()..]
        .split('(')
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn standard_handlers_preserve_values_errors_and_owned_receive_lists() {
    let mut output = source_output(include_str!("exercise.emel"));
    for name in [
        "gpio",
        "uart",
        "i2c",
        "spi",
        "clock",
        "console",
        "peripheral",
    ] {
        assert!(
            output
                .files
                .iter()
                .any(|file| file.path == format!("emela_{name}.h"))
        );
    }
    let aliases = [
        ("exercise_all", "all"),
        ("exercise_bounded", "bounded"),
        ("exercise_sleep_start", "sleep"),
        ("exercise_logs", "logs"),
    ]
    .map(|(alias, stem)| format!("#define {alias} {}\n", export_name(&output, stem)))
    .concat();
    output
        .files
        .iter_mut()
        .find(|file| file.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
size_t fixture_heap_live(void) {
  size_t used = 0;
  if (!emela_heap_initialized) return 0;
  for (unsigned char *p = emela_heap_storage; p < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)p;
    if (!block->is_free) used += block->size;
    p += block->size;
  }
  return used;
}
"#,
        );
    compile_and_run(output, &(aliases + include_str!("harness.c")));
}
