"""Generates a generics-heavy Emela program of N units on stdout, to measure what monomorphization
costs (crates/emela-core/NOTES.md, "JS の経路").

Each unit has a type and an enum with a type parameter, seven generic fns, and a `run` that uses
List.map / filter / fold / zip / head / find / any, Option and the shared `flat_map` at several
element types, nested generics and closures. main sums every unit's `run`.

    mkdir -p /tmp/gen50 && python3 scripts/gen-generics-bench.py 50 > /tmp/gen50/main.emel
    emela build /tmp/gen50/main.emel --out-dir /tmp/gen50/out && wc -c /tmp/gen50/out/main.mjs
"""

import sys

n = int(sys.argv[1])
out = []
w = out.append
w("# generated: generics-heavy program, %d units" % n)
w("""
fn flat_map[A, B](xs: List[A], f: fn(A) -> List[B]) -> List[B] {
  List.flatten(List.map(xs, f))
}

fn first_or[A](xs: List[A], d: A) -> A {
  List.head(xs) |> Option.or(d)
}

fn fst[A, B](p: (A, B)) -> A {
  (a, _) = p
  a
}

fn snd[A, B](p: (A, B)) -> B {
  (_, b) = p
  b
}

fn swap[A, B](p: (A, B)) -> (B, A) {
  (a, b) = p
  (b, a)
}
""")
for i in range(n):
    w(f"""
type Box{i}[T](value: T, tag: Int)

enum Tree{i}[T] {{
  Leaf{i}
  Node{i}(left: Tree{i}[T], value: T, right: Tree{i}[T])
}}

fn wrap{i}[T](x: T) -> Box{i}[T] {{
  Box{i}(value: x, tag: {i})
}}

fn unwrap{i}[T](xs: List[Box{i}[T]]) -> List[T] {{
  List.map(xs, fn(b) {{ b.value }})
}}

fn build{i}[T](xs: List[T]) -> Tree{i}[T] {{
  List.fold(xs, from: Leaf{i}, f: fn(t, x) {{ Node{i}(left: t, value: x, right: Leaf{i}) }})
}}

fn size{i}[T](t: Tree{i}[T]) -> Int {{
  t match {{
    Leaf{i} -> 0
    Node{i}(left:, value: _, right:) -> 1 + size{i}(left) + size{i}(right)
  }}
}}

fn to_list{i}[T](t: Tree{i}[T]) -> List[T] {{
  t match {{
    Leaf{i} -> []
    Node{i}(left:, value:, right:) -> List.append(to_list{i}(left), [value, ..to_list{i}(right)])
  }}
}}

fn pairs{i}[A, B](xs: List[A], ys: List[B]) -> List[(B, A)] {{
  List.map(List.zip(xs, ys), swap)
}}

fn count{i}[T](xs: List[T], keep: fn(T) -> Bool) -> Int {{
  List.length(List.filter(xs, keep))
}}

fn run{i}() -> Int {{
  ints = List.range(0, {i % 7 + 3})
  strs = List.map(ints, Int.to_string)
  floats = List.map(ints, Float.from_int)
  bools = List.map(ints, fn(x) {{ x % 2 == 0 }})
  boxes_i = List.map(ints, wrap{i})
  boxes_s = List.map(strs, wrap{i})
  boxes_f = List.map(floats, wrap{i})
  nested = List.map(boxes_i, fn(b) {{ wrap{i}(b) }})
  opts = List.map(strs, fn(s) {{ Some(s) }})
  t_i = build{i}(ints)
  t_s = build{i}(strs)
  t_p = build{i}(pairs{i}(ints, strs))
  t_o = build{i}(opts)
  lists = List.map(ints, fn(x) {{ List.range(0, x) }})
  flat = flat_map(lists, fn(l) {{ List.map(l, fn(x) {{ x * {i + 1} }}) }})
  words = flat_map(strs, fn(s) {{ [s, s] }})
  total = List.fold(flat, from: 0, f: fn(a, x) {{ a + x }})
  c = count{i}(bools, fn(b) {{ b }}) + count{i}(words, fn(s) {{ String.length(s) > 0 }})
    + count{i}(unwrap{i}(boxes_f), fn(x) {{ x > 1.0 }})
  sizes = size{i}(t_i) + size{i}(t_s) + size{i}(t_p) + size{i}(t_o)
  heads = first_or(unwrap{i}(boxes_i), 0) + String.length(first_or(unwrap{i}(boxes_s), ""))
    + first_or(List.map(unwrap{i}(nested), fn(b) {{ b.tag }}), 0)
  found = List.find(to_list{i}(t_p), fn(p) {{ snd(p) == 1 }}) |> Option.map(fn(p) {{ String.length(fst(p)) }}) |> Option.or(0)
  anys = if List.any(to_list{i}(t_o), Option.is_some) {{ 1 }} else {{ 0 }}
  total + c + sizes + heads + found + anys
}}
""")
w("fn main() {")
w("  results = [" + ", ".join(f"run{i}()" for i in range(n)) + "]")
w("  dbg(List.fold(results, from: 0, f: fn(a, x) { a + x }))")
w("}")
print("\n".join(out))
