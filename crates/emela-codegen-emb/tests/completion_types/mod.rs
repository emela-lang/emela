use super::*;

mod derived;

fn export_name(output: &Output, stem: &str) -> String {
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let prefix = format!("emela_export_{stem}_");
    let line = header.lines().find(|line| line.contains(&prefix)).unwrap();
    line[line.find(&prefix).unwrap()..]
        .split('(')
        .next()
        .unwrap()
        .to_owned()
}

fn add_heap_check(output: &mut Output) {
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
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
}

#[test]
fn finite_changing_recursive_enum_arguments_run_with_owned_back_edges() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
pub enum Nest[A] { End(A), Next(Nest[Bool]) }
fn count(value: Nest[Bool]) -> Int {
  value match { End(ok) -> if ok { 1 } else { 0 }
    Next(rest) -> count(rest) + 1 }
}
pub fn work(n: Int) -> Int {
  value: Nest[Int] = if n == 0 { End(42) } else { Next(Next(End(n == 1))) }
  value match { End(number) -> number
    Next(rest) -> count(rest) + 1 }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(&core, &Options::default().with_heap_bytes(4096))
        .expect("finite changing recursive enum types");
    let run = export_name(&output, "work");
    add_heap_check(&mut output);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {{ (void)k;(void)s;(void)n;abort(); }}
int main(void) {{for(unsigned i=0;i<1000;i++) {{assert({run}(0)==42);assert(test_heap_empty());assert({run}(1)==3);assert(test_heap_empty());}}return 0;}}
"#
        ),
    );
}

fn closed_set_function_flow(mode: &str) -> Module {
    let mut core = emela_core::Module::new();
    let errors = core.intern(Type::Set(emela_core::SetSummary::default()));
    let effects = match mode {
        "unresolved" => {
            let p = core.type_param(0);
            core.intern(Type::Set(emela_core::SetSummary {
                params: vec![p],
                ..Default::default()
            }))
        }
        "malformed" => TypeId::INT,
        _ => errors,
    };
    let fn_ty = core.intern(Type::Fn(emela_core::FnType {
        params: vec![TypeId::INT],
        ret: TypeId::INT,
        fails: mode == "inconsistent",
        ev: false,
        suspends: false,
        sets: Some(emela_core::FnSets { errors, effects }),
    }));
    let mut flow = Module::new(core);
    let mut inc = FnBuilder::new(&flow.core, "increment");
    let input = inc.param("input", TypeId::INT);
    inc.set_ret(TypeId::INT);
    let value = inc.assign(
        "next",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![input.into(), Const::Int(1).into()],
        },
    );
    inc.term(Term::Return(Some(value.into())));
    let inc = flow.add(inc.finish());
    let mut b = FnBuilder::new(&flow.core, "sets");
    b.set_exported().set_ret(TypeId::INT);
    let f = b.assign("f", fn_ty, Value::FnRef(inc));
    let value = b.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Indirect(f),
            args: vec![Const::Int(41).into()],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    flow
}

#[test]
fn concrete_function_sets_are_normalized_before_value_abi_validation() {
    let mut flow = closed_set_function_flow("closed");
    let closed = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "sets")
        .unwrap()
        .1
        .vars
        .iter()
        .find(|(_, v)| matches!(flow.core.types[v.ty], Type::Fn(_)))
        .unwrap()
        .1
        .ty;
    let mut unused = FnBuilder::new(&flow.core, "unused");
    unused.param("f", closed);
    unused.set_ret(TypeId::INT);
    unused.term(Term::Return(Some(Const::Int(0).into())));
    flow.add(unused.finish());
    let output = compile_flow(&flow, &Options::default()).expect("closed sets have an exact ABI");
    compile_and_run(
        output,
        "#include <assert.h>\n#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind k,const char*s,uint16_t n){(void)k;(void)s;(void)n;abort();}\nint main(void){assert(emela_export_sets_1()==42);return 0;}\n",
    );
}

#[test]
fn unresolved_and_inconsistent_function_sets_have_explicit_invalid_flow_diagnostics() {
    for (mode, reason) in [
        ("unresolved", "unresolved function error/effect set"),
        ("inconsistent", "function ABI flags disagree"),
        ("malformed", "not set summaries"),
    ] {
        let flow = closed_set_function_flow(mode);
        let errors = compile_flow(&flow, &Options::default()).expect_err(mode);
        assert!(
            errors
                .iter()
                .any(|error| error.kind == ErrorKind::InvalidFlowIr
                    && error.message.contains(reason)),
            "{errors:?}"
        );
    }
}

#[test]
fn growing_enum_opaque_edges_trace_cycles_through_cell_and_any() {
    let mut core = emela_core::Module::new();
    let p = core.type_param(0);
    let id = core.add_enum(EnumDef {
        name: "Grow".into(),
        params: 1,
        variants: vec![],
        error: false,
        external: false,
    });
    let list_p = core.list(p);
    let next = core.enum_type(id, vec![list_p]);
    core.enums[id].variants = vec![
        VariantDef::positional("End", vec![p]),
        VariantDef::positional("Next", vec![next]),
    ];
    let cell_ty = core.cell(TypeId::ANY);
    let wrap_ty = core.enum_type(id, vec![cell_ty]);
    let mut b = FnBuilder::new(&core, "cycle");
    b.set_exported().set_ret(TypeId::INT);
    let n = b.assign("n", TypeId::INT, Value::Const(Const::Int(42)));
    let initial = b.assign("initial", TypeId::ANY, Value::Coerce(n));
    let cell = b.assign(
        "cell",
        cell_ty,
        Value::Builtin {
            op: Builtin::CellNew,
            args: vec![initial.into()],
        },
    );
    let wrap = b.assign(
        "wrap",
        wrap_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: id,
                variant: 0,
            },
            args: vec![cell.into()],
        },
    );
    let boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(wrap));
    b.let_(
        None,
        Value::Builtin {
            op: Builtin::CellSet,
            args: vec![cell.into(), boxed.into()],
        },
    );
    b.term(Term::Return(Some(n.into())));
    let function = b.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(4096)).unwrap();
    add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char*s,uint16_t n){(void)k;(void)s;(void)n;abort();}
int main(void){for(int i=0;i<1000;i++){assert(emela_export_cycle_0()==42);assert(test_heap_empty());}return 0;}
"#,
    );
}
