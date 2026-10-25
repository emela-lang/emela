use super::*;
#[path = "../../examples/support/external_scalar_flow.rs"]
mod fixture;

#[test]
fn native_external_int64_and_float_calls_preserve_bits_without_heap() {
    let flow = fixture::flow();
    for entry in ["direct_integer", "direct_real"] {
        let errors = compile_flow(
            &flow,
            &Options::default().with_heap_bytes(0).with_isr_entry(entry),
        )
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("not marked ISR-safe"))
        );
    }
    let options = Options::default()
        .with_heap_bytes(0)
        .with_isr_entry("direct_integer")
        .with_isr_entry("direct_real")
        .with_isr_safe_external("board_integer")
        .with_isr_safe_external("board_real");
    let output = compile_flow(&flow, &options).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("emela_heap_storage")
    );
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
int64_t board_integer(int64_t n) { return n; }
double board_real(double n) { return n; }
static uint64_t bits(double n) { uint64_t out;memcpy(&out,&n,sizeof(out));return out; }
int main(void) {
  static const int64_t integers[]={INT64_MIN,INT64_MAX,-INT64_C(9007199254740993),INT64_C(9007199254740993),-1,0,1};
  static const uint64_t floats[]={UINT64_C(0),UINT64_C(0x8000000000000000),UINT64_C(1),UINT64_C(0x8000000000000001),
    UINT64_C(0x0010000000000000),UINT64_C(0x7fefffffffffffff),UINT64_C(0x7ff0000000000000),UINT64_C(0xfff0000000000000),
    UINT64_C(0x7ff8000000000042),UINT64_C(0xfff8000000001234),UINT64_C(0x3ff123456789abcd)};
  for(int i=0;i<1000;i++) {
    for(unsigned j=0;j<sizeof(integers)/sizeof(*integers);j++) {
      assert(emela_export_direct_integer_2(integers[j])==integers[j]);
      assert(emela_export_indirect_integer_4(integers[j])==integers[j]);
    }
    for(unsigned j=0;j<sizeof(floats)/sizeof(*floats);j++) {
      double n;memcpy(&n,&floats[j],sizeof(n));
      assert(bits(emela_export_direct_real_3(n))==floats[j]);
      assert(bits(emela_export_indirect_real_5(n))==floats[j]);
    }
  }
}
"#,
    );
    let errors = compile_flow(&flow, &options.with_isr_entry("indirect_integer")).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("dynamic call reachable"))
    );
}

#[test]
fn source_handler_can_call_native_external_int64_and_float_with_mixed_arguments() {
    let source = r#"
@external(c, "board_mix")
fn mix(a: Int64, b: Float) -> Float
effect Sensor { fn read(a: Int64, b: Float) -> Float }
handler BoardSensor: Sensor { fn read(a,b) { mix(a,b) } }
pub fn read(a: Int64, b: Float) -> Float { with BoardSensor { (use Sensor).read(a,b) } }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Public).unwrap();
    let output = compile(&core, &Options::default().with_heap_bytes(1024)).unwrap();
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
double board_mix(int64_t a,double b) { assert(a==INT64_MIN);return b; }
int main(void) { for(int i=0;i<1000;i++) {
  const uint64_t expected=UINT64_C(0xfff8000000000042);uint64_t actual;double n;
  memcpy(&n,&expected,sizeof(n));double result=emela_export_read_0(INT64_MIN,n);
  memcpy(&actual,&result,sizeof(actual));assert(actual==expected);
}}
"#,
    );
}
