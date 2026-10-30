use super::*;

const PRELUDE: &str = r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {
  fprintf(stderr,"defect %d: %.*s\n",k,n,s); abort();
}
"#;

fn name(output: &Output, stem: &str) -> String {
    let prefix = format!("emela_export_{stem}_");
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let declaration = header.lines().find(|line| line.contains(&prefix)).unwrap();
    declaration[declaration.find(&prefix).unwrap()..]
        .split('(')
        .next()
        .unwrap()
        .into()
}

fn fixture(source: &str, options: Options) -> Output {
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    compile(&core, &options).unwrap()
}

fn run(mut output: Output, harness: &str, expected: Option<&str>) {
    let source = &mut output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    if source.contains("emela_heap_storage") {
        source.push_str(
            r#"
int test_heap_empty(void) {
  if(!emela_heap_initialized) return 1;
  for(unsigned char *p=emela_heap_storage;p<emela_heap_end();) {
    emela_heap_block *b=(emela_heap_block *)(void *)p;
    if(!b->is_free) return 0;
    p+=b->size;
  }
  return 1;
}
"#,
        );
        // Only scheduler tests need access to the internal fiber handle.
        if source.contains("emela_fiber_get(") {
            source
                .push_str("void test_cancel(EmelaFiber f) { emela_fiber_get(f)->cancelled=1; }\n");
        }
    } else {
        source.push_str("\nint test_heap_empty(void) { return 1; }\n");
    }
    let temp = TempDir::new();
    for file in output.files {
        fs::write(temp.0.join(file.path), file.contents).unwrap();
    }
    let lifecycle = "\nvoid emela_platform_start(void (*program)(void)) { program(); }\nvoid emela_board_init(void) {}\nvoid emela_board_idle(void) {}\n";
    fs::write(temp.0.join("harness.c"), format!("{harness}{lifecycle}")).unwrap();
    for optimization in ["-O0", "-O2"] {
        let executable = temp.0.join("run");
        let built = Command::new("cc")
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                optimization,
                "-I",
            ])
            .arg(&temp.0)
            .arg(temp.0.join("emela_embedded.c"))
            .arg(temp.0.join("harness.c"))
            .args(["-lm", "-o"])
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let result = Command::new(executable).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if let Some(expected) = expected {
            assert_eq!(String::from_utf8(result.stdout).unwrap(), expected);
        }
    }
}

#[test]
fn bytes_std_corpus_matches_other_targets_including_const_traits_and_invalid_utf8() {
    // Keep a local copy of the cross-target corpus, bounded to the embedded String capacity.
    // Separate span tests cover byte lengths above u16 without converting them to String.
    let source = include_str!("fixtures/bytes.emel")
        .replace("fn main()", "pub fn main()")
        .replace("20000", "128");
    let output = fixture(
        &source,
        Options::default()
            .with_heap_bytes(131072)
            .with_string_capacity(512),
    );
    let main = name(&output, "main");
    run(
        output,
        &format!(
            r#"{PRELUDE}
void emela_board_debug(const char *s,uint16_t n) {{ fwrite(s,1,n,stdout); putchar('\n'); }}
int main(void) {{ {main}(); return 0; }}
"#
        ),
        Some(
            &include_str!("fixtures/bytes.expected")
                .split_once("--- stderr\n")
                .unwrap()
                .1
                .replace("20000\n", "128\n"),
        ),
    );
}

#[test]
fn bytes_size_and_get_are_heapless_and_keep_lengths_above_u16() {
    let output = fixture(
        r#"
import Std.Bytes.{Bytes}
pub fn size(b: Bytes) -> Int { Bytes.size(b) }
pub fn at(b: Bytes, i: Int) -> Int { Option.or(Bytes.get(b, i), -1) }
"#,
        Options::default().with_isr_entry("size"),
    );
    let size = name(&output, "size");
    let at = name(&output, "at");
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("emela_heap_storage")
    );
    run(
        output,
        &format!(
            r#"{PRELUDE}
int main(void) {{
  static uint8_t data[70000]; data[0]=128;data[69999]=255;
  EmelaBytes b={{data,70000,NULL}};
  assert({size}(b)==70000);assert({at}(b,0)==128);assert({at}(b,69999)==255);
  assert({at}(b,-1)==-1);assert({at}(b,70000)==-1);
  assert({size}((EmelaBytes){{0}})==0);return 0;
}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_returns_lists_and_aliases_preserve_ownership_and_release_the_heap() {
    let output = fixture(
        r#"
import Std.Bytes.{Bytes}
pub fn make(s: String) -> Bytes { Bytes.encode_utf8(s) }
pub fn copy(b: Bytes) -> Bytes { b }
pub fn part(b: Bytes, a: Int, z: Int) -> Bytes { Bytes.slice(b, a, z) }
pub fn joined(b: Bytes) -> Bytes { Bytes.concat([b, b]) }
pub fn listed(b: Bytes) -> List[Bytes] { [b] }
"#,
        Options::default()
            .with_heap_bytes(4096)
            .with_string_capacity(1),
    );
    let make = name(&output, "make");
    let copy = name(&output, "copy");
    let part = name(&output, "part");
    let joined = name(&output, "joined");
    let listed = name(&output, "listed");
    run(
        output,
        &format!(
            r#"{PRELUDE}
int main(void) {{for(unsigned i=0;i<500;i++) {{
  char text[]={{'a',0,'z'}}; EmelaBytes b={make}((EmelaString){{text,3,NULL}});
  memset(text,'x',3); assert(b.length==3 && b.data[0]=='a' && b.data[1]==0);
  EmelaBytes c={copy}(b),s={part}(b,INT32_MIN,INT32_MAX),j={joined}(b);
  void *list={listed}(b); emela_embedded_release_bytes(&b);
  assert(c.data[2]=='z' && s.length==3 && j.length==6 && j.data[4]==0);
  emela_embedded_release(list); emela_embedded_release_bytes(&c);
  emela_embedded_release_bytes(&s);emela_embedded_release_bytes(&j);
  uint8_t borrowed[]={{0,128,255}};c={copy}((EmelaBytes){{borrowed,3,NULL}});
  memset(borrowed,1,3);assert(c.data[1]==128 && c.data[2]==255);
  emela_embedded_release_bytes(&c);assert(test_heap_empty());
}} return 0;}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_dynamic_capacity_is_independent_of_string_capacity() {
    let output = fixture(
        r#"
import Std.Bytes.{Bytes}
pub fn part(b: Bytes) -> Bytes { Bytes.slice(b, -1, 100000) }
"#,
        Options::default()
            .with_heap_bytes(262144)
            .with_string_capacity(1),
    );
    let part = name(&output, "part");
    run(
        output,
        &format!(
            r#"{PRELUDE}
int main(void) {{
 static uint8_t input[70000];for(unsigned i=0;i<sizeof input;i++) input[i]=(uint8_t)i;
 for(unsigned i=0;i<3;i++) {{
   EmelaBytes out={part}((EmelaBytes){{input,sizeof input,NULL}});
   assert(out.length==sizeof input && memcmp(out.data,input,sizeof input)==0);
   emela_embedded_release_bytes(&out);assert(test_heap_empty());
 }}return 0;
}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_survive_suspension_and_release_on_completion_and_cancellation() {
    let output = fixture(
        r#"
import Std.Bytes.{Bytes}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting: Wait { suspend fn get(n, resume) { resume(wait(n)) } }
pub fn across(b: Bytes) -> Int use Async {
  with Waiting {
    snapshot = Bytes.slice(b, 0, Bytes.size(b))
    (use Wait).get(1)
    Option.or(Bytes.get(b, 0), 0) + Option.or(Bytes.get(snapshot, 2), 0)
  }
}
"#,
        Options::default()
            .with_heap_bytes(4096)
            .with_fiber_capacity(2)
            .with_frame_capacity(8)
            .with_frame_bytes(512)
            .with_task_capacity(1)
            .with_event_capacity(1)
            .with_deferred_capacity(0)
            .with_task_adapter("host_wait", "board_wait_start"),
    );
    let across = name(&output, "across");
    run(
        output,
        &format!(
            r#"{PRELUDE}
void test_cancel(EmelaFiber f);
static EmelaTask task;
uint32_t emela_board_event_lock(void) {{return 0;}}
void emela_board_event_unlock(uint32_t n) {{(void)n;}}
void board_wait_start(EmelaTask t,int32_t n) {{assert(n==1);task=t;}}
static void drain(void) {{while(emela_scheduler_run(1)) {{}}}}
int main(void) {{for(unsigned i=0;i<200;i++) {{for(unsigned route=0;route<3;route++) {{
  uint8_t b[]={{1,0,255}};int32_t result=-1;
  EmelaFiber f={across}((EmelaBytes){{b,3,NULL}},route==1?NULL:&result);
  memset(b,9,3);drain();
  if(route==2) {{test_cancel(f);drain();}}
  else {{assert(emela_task_complete_int(task,1));drain();if(route==0) assert(result==256);}}
  emela_fiber_release(f);assert(test_heap_empty());
}}}}return 0;}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_captured_in_closures_and_state_keep_their_owners() {
    let output = fixture(
        r#"
import Std.Bytes.{Bytes}
import Std.State.{State}
pub fn captured(s: String) -> Bytes {
  value = Bytes.encode_utf8(s)
  read = fn() { value }
  read()
}
pub fn state(b: Bytes) -> Bytes {
  (value, final) = State.run(b, fn() {
    state = use State[Bytes]
    state.set(Bytes.append(state.get(), state.get()))
    state.get()
  })
  value
}
"#,
        Options::default().with_heap_bytes(8192),
    );
    let captured = name(&output, "captured");
    let state = name(&output, "state");
    run(
        output,
        &format!(
            r#"{PRELUDE}
int main(void) {{for(unsigned i=0;i<500;i++) {{
  char input[]={{'a',0,'b'}};EmelaBytes b={captured}((EmelaString){{input,3,NULL}});
  memset(input,'x',3);assert(b.length==3 && b.data[0]=='a');
  EmelaBytes c={state}(b);emela_embedded_release_bytes(&b);
  assert(c.length==6 && c.data[0]=='a' && c.data[4]==0);
  emela_embedded_release_bytes(&c);assert(test_heap_empty());
}}return 0;}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_finite_flow_derivations_compare_and_display_unsigned_bytes() {
    let mut flow = Module::new(emela_core::Module::new());
    let eq = flow.add(
        emela_codegen_emb::derived_binding(
            &flow.core,
            emela_codegen_emb::DerivedOperation::Eq,
            TypeId::BYTES,
            "bytes_eq",
        )
        .unwrap(),
    );
    let show = flow.add(
        emela_codegen_emb::derived_binding(
            &flow.core,
            emela_codegen_emb::DerivedOperation::Show,
            TypeId::BYTES,
            "bytes_show",
        )
        .unwrap(),
    );
    let mut builder = FnBuilder::new(&flow.core, "equal");
    builder.set_exported().set_ret(TypeId::BOOL);
    let a = builder.param("a", TypeId::BYTES);
    let b = builder.param("b", TypeId::BYTES);
    let result = builder.assign(
        "result",
        TypeId::BOOL,
        Value::Call {
            callee: Callee::Direct(eq),
            args: vec![a.into(), b.into()],
            on_fail: None,
            suspends: false,
        },
    );
    builder.term(Term::Return(Some(result.into())));
    flow.add(builder.finish());
    let mut builder = FnBuilder::new(&flow.core, "show");
    builder.set_exported().set_ret(TypeId::STRING);
    let a = builder.param("a", TypeId::BYTES);
    let result = builder.assign(
        "result",
        TypeId::STRING,
        Value::Call {
            callee: Callee::Direct(show),
            args: vec![a.into()],
            on_fail: None,
            suspends: false,
        },
    );
    builder.term(Term::Return(Some(result.into())));
    flow.add(builder.finish());
    let output = compile_flow(&flow, &Options::default().with_string_capacity(64)).unwrap();
    let equal = name(&output, "equal");
    let show = name(&output, "show");
    run(
        output,
        &format!(
            r#"{PRELUDE}
int main(void) {{
  uint8_t a[]={{0,128,255}},b[]={{0,128,254}};char buffer[64];
  EmelaBytes x={{a,3,NULL}},y={{b,3,NULL}};
  assert({equal}(x,x));assert(!{equal}(x,y));
  EmelaString text={show}(x,buffer,sizeof buffer);
  const char *expected="Bytes.hex(\"0080ff\")";
  assert(text.length==strlen(expected) && memcmp(text.data,expected,text.length)==0);
  return 0;
}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_external_adapters_snapshot_borrowed_data_and_transfer_owned_data() {
    for ownership in [
        emela_codegen_emb::ExternalResult::Borrowed,
        emela_codegen_emb::ExternalResult::Owned,
    ] {
        let output = fixture(
            r#"
import Std.Bytes.{Bytes}
@external(c, "raw")
fn raw(n: Int) -> Bytes use {}
pub fn work(n: Int) -> Bytes { raw(n) }
"#,
            Options::default()
                .with_heap_bytes(4096)
                .with_external_adapter(
                    "raw",
                    emela_codegen_emb::ExternalAdapter::new("native_raw", ownership),
                ),
        );
        let work = name(&output, "work");
        let acquire = if ownership == emela_codegen_emb::ExternalResult::Owned {
            "emela_external_own_10(&result);"
        } else {
            ""
        };
        run(
            output,
            &format!(
                r#"{PRELUDE}
EmelaBytes native_raw(int32_t n) {{
  static uint8_t buffer[3];buffer[0]=(uint8_t)n;buffer[1]=0;buffer[2]=255;
  EmelaBytes result={{buffer,3,NULL}};{acquire}return result;
}}
int main(void) {{for(unsigned i=0;i<500;i++) {{
  EmelaBytes a={work}(1),b={work}(2);
  assert(a.data[0]==1 && b.data[0]==2 && a.data[2]==255);
  emela_embedded_release_bytes(&a);emela_embedded_release_bytes(&b);assert(test_heap_empty());
}}return 0;}}
"#
            ),
            None,
        );
    }
}

#[test]
fn bytes_require_a_fixed_heap_and_defect_when_its_capacity_is_exhausted() {
    let source = r#"import Std.Bytes.{Bytes}
pub fn make(s: String) -> Bytes { Bytes.encode_utf8(s) }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    let errors = compile(&core, &Options::default()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == ErrorKind::Unsupported && e.message.contains("heap"))
    );
    let output = compile(&core, &Options::default().with_heap_bytes(128)).unwrap();
    let make = name(&output, "make");
    let prelude = PRELUDE.replace("abort();", "exit(k==EMELA_DEFECT_HEAP_EXHAUSTED ? 0 : 3);");
    run(
        output,
        &format!(
            r#"{prelude}
int main(void) {{static char data[1024];(void){make}((EmelaString){{data,sizeof data,NULL}});return 2;}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_decoding_checks_string_capacity_before_truncating_the_length() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut builder = FnBuilder::new(&flow.core, "decode");
    builder.set_exported().set_ret(TypeId::STRING);
    let input = builder.param("input", TypeId::BYTES);
    let result = builder.assign(
        "result",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::BytesDecodeValidUtf8,
            args: vec![input.into()],
        },
    );
    builder.term(Term::Return(Some(result.into())));
    flow.add(builder.finish());
    let output = compile_flow(&flow, &Options::default().with_string_capacity(8)).unwrap();
    let decode = name(&output, "decode");
    let prelude = PRELUDE.replace("abort();", "exit(k==EMELA_DEFECT_STRING_CAPACITY ? 0 : 3);");
    run(
        output,
        &format!(
            r#"{prelude}
int main(void) {{static uint8_t data[70000];char buffer[8];(void){decode}((EmelaBytes){{data,sizeof data,NULL}},buffer,sizeof buffer);return 2;}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_builtins_reject_reference_arguments_with_the_wrong_semantic_type() {
    for op in [
        Builtin::BytesFromList,
        Builtin::BytesToList,
        Builtin::BytesSize,
        Builtin::BytesGet,
        Builtin::BytesSlice,
        Builtin::BytesConcat,
        Builtin::BytesEncodeUtf8,
        Builtin::BytesDecodeValidUtf8,
    ] {
        let mut flow = Module::new(emela_core::Module::new());
        let mut builder = FnBuilder::new(&flow.core, "invalid");
        builder.set_exported();
        let wrong = builder.param("wrong", TypeId::ANY);
        let mut args = vec![wrong.into()];
        if matches!(op, Builtin::BytesGet | Builtin::BytesSlice) {
            args.push(Const::Int(0).into());
        }
        if op == Builtin::BytesSlice {
            args.push(Const::Int(1).into());
        }
        builder.let_(None, Value::Builtin { op, args });
        builder.term(Term::Return(None));
        flow.add(builder.finish());
        let errors = compile_flow(&flow, &Options::default().with_heap_bytes(4096)).unwrap_err();
        assert!(
            errors.iter().all(|e| e.kind == ErrorKind::InvalidFlowIr),
            "{op:?}: {errors:?}"
        );
    }
}

#[test]
fn bytes_list_round_trips_and_invalid_elements_reclaim_every_allocation() {
    let output = fixture(
        r#"
import Std.Bytes.{Bytes}
pub fn round_trip(n: Int) -> Bool {
  value = Option.or(Bytes.from_list([0, n, 255]), Bytes.empty())
  if n < 0 || n > 255 { Bytes.is_empty(value) }
  else { Bytes.to_list(value) == [0, n, 255] }
}
"#,
        Options::default().with_heap_bytes(4096),
    );
    let round_trip = name(&output, "round_trip");
    run(
        output,
        &format!(
            r#"{PRELUDE}
int main(void) {{for(unsigned i=0;i<500;i++) {{
  assert({round_trip}(0));assert(test_heap_empty());
  assert({round_trip}(128));assert(test_heap_empty());
  assert({round_trip}(255));assert(test_heap_empty());
  assert({round_trip}(-1));assert(test_heap_empty());
  assert({round_trip}(256));assert(test_heap_empty());
}}return 0;}}
"#
        ),
        None,
    );
}

#[test]
fn discarded_bytes_still_check_capacity_and_release_owned_results() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Bytes.{Bytes}
pub fn discard(s: String) { Bytes.encode_utf8(s)
  () }
"#,
        )],
        Roots::Public,
    )
    .unwrap();
    let output = compile(&core, &Options::default().with_heap_bytes(128)).unwrap();
    let discard = name(&output, "discard");
    let prelude = PRELUDE.replace("abort();", "exit(k==EMELA_DEFECT_HEAP_EXHAUSTED ? 0 : 3);");
    run(
        output,
        &format!(
            r#"{prelude}
int main(void) {{static char input[1024];{discard}((EmelaString){{input,sizeof input,NULL}});return 2;}}
"#
        ),
        None,
    );
    let output = compile(&core, &Options::default().with_heap_bytes(256)).unwrap();
    let discard = name(&output, "discard");
    run(
        output,
        &format!(
            r#"{PRELUDE}
int main(void) {{for(unsigned i=0;i<1000;i++) {{{discard}((EmelaString){{"abc",3,NULL}});assert(test_heap_empty());}}return 0;}}
"#
        ),
        None,
    );
}

#[test]
fn bytes_external_tasks_snapshot_arguments_and_results_and_release_cancelled_contexts() {
    let output = fixture(
        r#"
import Std.Bytes.{Bytes}
@external(c, "host_bytes")
suspend fn exchange(b: Bytes) -> Bytes
effect Exchange { suspend fn bytes(b: Bytes) -> Bytes }
handler Exchanging: Exchange { suspend fn bytes(b, resume) { resume(exchange(b)) } }
pub fn echoed(b: Bytes) -> Bytes use Async {
  with Exchanging { (use Exchange).bytes(b) }
}
"#,
        Options::default()
            .with_heap_bytes(4096)
            .with_frame_bytes(1024)
            .with_task_adapter("host_bytes", "board_bytes_start"),
    );
    let echoed = name(&output, "echoed");
    run(
        output,
        &format!(
            r#"{PRELUDE}
void test_cancel(EmelaFiber f);
static EmelaTask pending;
static uint8_t board_bytes[3];
static int produced,released,locked,isr;
uint32_t emela_board_event_lock(void) {{ int previous=locked;locked=1;return (uint32_t)previous; }}
void emela_board_event_unlock(uint32_t p) {{ assert(locked);locked=(int)p; }}
void board_bytes_start(EmelaTask task,EmelaBytes input) {{
  assert(!locked&&!isr&&input.owner&&input.length==3);
  pending=task;memcpy(board_bytes,input.data,3);
}}
static void produce(void *context,EmelaBytes *out) {{
  assert(!locked&&!isr);produced++;*out=(EmelaBytes){{context,3,NULL}};
}}
static void release(void *context) {{ assert(!locked&&!isr);released++;memset(context,1,3); }}
static void drain(void) {{ while(emela_scheduler_run(1)) {{}} }}
int main(void) {{ for(int i=0;i<500;i++) {{
  uint8_t input[]={{0,128,255}};EmelaBytes result={{0}};produced=released=0;
  int cancel=i%5==1,discard=i%3==0;
  EmelaFiber f={echoed}((EmelaBytes){{input,3,NULL}},discard?NULL:&result);
  memset(input,1,3);drain();assert(board_bytes[0]==0&&board_bytes[1]==128&&board_bytes[2]==255);
  isr=1;
  assert(!emela_task_complete_int(pending,0));
  assert(emela_task_complete_value_10(pending,produce,board_bytes,release));
  assert(!emela_task_complete_value_10(pending,produce,board_bytes,release));
  assert(!produced&&!released);isr=0;
  if(cancel) {{ test_cancel(f); }}
  drain();assert(emela_fiber_done(f));emela_fiber_release(f);
  assert(produced==!cancel&&released==1);
  if(!discard&&!cancel) {{
    assert(result.owner&&result.length==3&&result.data[0]==0&&result.data[1]==128&&result.data[2]==255);
    emela_embedded_release_bytes(&result);
  }}
  assert(test_heap_empty());
}} return 0; }}
"#
        ),
        None,
    );
}
