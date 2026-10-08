//! Core IR から JS のテキストを出す．
//!
//! 式は `value`（JS の式を返す．前置きの文を出してよい）と `stmt`（値を行き先へ渡す文を出す）
//! の2通りで出す．`let` / `match` / ループなど文が要る式は，一時変数に代入してから式として使う．
//! 文を前に出すと，それより左にある引数の評価が後ろへずれるので，その前に左の引数を一時変数に束縛して
//! 左から右の評価順を保つ．

use std::collections::BTreeSet;
use std::fmt::Write as _;

use emela_core::*;

use crate::{Options, RUNTIME_FILE, RuntimeMode, inline_runtime};

/// 値の行き先．
#[derive(Clone)]
enum Dest {
    Return,
    Assign(String),
}

struct LoopCtx {
    label: String,
    cells: Vec<String>,
}

pub(crate) struct Emitter<'m> {
    m: &'m Module,
    out: String,
    indent: usize,
    next_tmp: u32,
    next_label: u32,
    used: BTreeSet<&'static str>,
    loops: Vec<LoopCtx>,
    /// 表示関数を出す enum．出したものも残す（`shows_done` まで出し終えた）．
    shows: Vec<EnumId>,
    shows_done: usize,
}

pub(crate) fn emit_module(m: &Module, opts: &Options) -> String {
    let mut e = Emitter {
        m,
        out: String::new(),
        indent: 0,
        next_tmp: 0,
        next_label: 0,
        used: BTreeSet::new(),
        loops: Vec::new(),
        shows: Vec::new(),
        shows_done: 0,
    };

    // 構成子を持たないバリアントは1つの値を共有する．
    for (id, def) in m.enums.iter() {
        if m.option == Some(id) {
            // Prelude の Option の None はランタイムが返すものと同じ値にする．
            assert!(
                def.variants.len() == 2 && def.variants[OPTION_NONE].fields.is_empty(),
                "不正な IR: Prelude の Option の形が違う"
            );
            let none = e.rt("$None");
            e.line(&format!(
                "const {} = {none};",
                unit_variant_name(&def.name, &def.variants[OPTION_NONE].name)
            ));
            continue;
        }
        for (k, v) in def.variants.iter().enumerate() {
            if v.fields.is_empty() {
                e.line(&format!(
                    "const {} = Object.freeze({{ $tag: {k} }});",
                    unit_variant_name(&def.name, &v.name)
                ));
            }
        }
    }
    if !m.enums.is_empty() {
        e.line("");
    }

    for (id, f) in m.functions.iter() {
        let params: Vec<String> = f.params.iter().map(|&p| e.local(p)).collect();
        e.line(&format!(
            "function {}({}) {{",
            fn_name(&m.functions[id].name),
            params.join(", ")
        ));
        e.indent += 1;
        e.stmt(&f.body, &Dest::Return);
        e.indent -= 1;
        e.line("}");
        e.line("");
    }

    // 表示関数は，表示関数の中で別の enum の表示が要ることがあるので，尽きるまで出す．
    while e.shows_done < e.shows.len() {
        let id = e.shows[e.shows_done];
        e.shows_done += 1;
        e.show_enum_fn(id);
        e.line("");
    }

    let exports: Vec<String> = m
        .functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(_, f)| {
            let js = fn_name(&f.name);
            if js == f.name {
                js
            } else {
                format!("{js} as {}", f.name)
            }
        })
        .collect();
    if !exports.is_empty() {
        e.line(&format!("export {{ {} }};", exports.join(", ")));
    }

    let mut head = String::from("// Emela が生成したコード．\n");
    match &opts.runtime {
        RuntimeMode::Import(spec) => {
            if !e.used.is_empty() {
                let names: Vec<&str> = e.used.iter().copied().collect();
                let spec = spec.as_deref().unwrap_or(RUNTIME_FILE);
                let _ = writeln!(
                    head,
                    "import {{ {} }} from {};",
                    names.join(", "),
                    js_string(&format!("./{spec}"))
                );
            }
        }
        RuntimeMode::Inline => {
            head.push_str(&inline_runtime());
            head.push_str("// ここから生成コード．\n");
        }
    }
    head.push('\n');
    head + &e.out
}

impl Emitter<'_> {
    fn line(&mut self, s: &str) {
        if s.is_empty() {
            self.out.push('\n');
            return;
        }
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn rt(&mut self, name: &'static str) -> &'static str {
        self.used.insert(name);
        name
    }

    fn tmp(&mut self) -> String {
        let t = format!("$t{}", self.next_tmp);
        self.next_tmp += 1;
        t
    }

    fn label(&mut self, prefix: &str) -> String {
        let l = format!("${prefix}{}", self.next_label);
        self.next_label += 1;
        l
    }

    fn local(&self, l: Local) -> String {
        let hint: String = self.m.locals[l]
            .hint
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("{hint}${}", l.into_raw().into_u32())
    }

    fn emit_dest(&mut self, dest: &Dest, v: &str) {
        match dest {
            Dest::Return => self.line(&format!("return {v};")),
            Dest::Assign(t) => self.line(&format!("{t} = {v};")),
        }
    }

    /// `es` を左から順に評価した JS の式を返す．
    fn values_in_order(&mut self, es: &[&Expr]) -> Vec<String> {
        let last_complex = es.iter().rposition(|e| !is_simple(e));
        let mut vs = Vec::with_capacity(es.len());
        for (i, e) in es.iter().enumerate() {
            let v = self.value(e);
            if last_complex.is_some_and(|k| i < k) && !is_atom(e) {
                let t = self.tmp();
                self.line(&format!("const {t} = {v};"));
                vs.push(t);
            } else {
                vs.push(v);
            }
        }
        vs
    }

    /// 文が要る式を一時変数に入れて，その名前を返す．
    fn via_tmp(&mut self, e: &Expr) -> String {
        let t = self.tmp();
        self.line(&format!("let {t};"));
        self.stmt(e, &Dest::Assign(t.clone()));
        t
    }

    fn value(&mut self, e: &Expr) -> String {
        match e {
            Expr::Lit(lit) => lit_js(lit),
            Expr::Var(l) => self.local(*l),
            Expr::Fn(f) => fn_name(&self.m.functions[*f].name),
            Expr::Let { var, value, body } => {
                let v = self.value(value);
                let name = self.local(*var);
                self.line(&format!("const {name} = {v};"));
                self.value(body)
            }
            Expr::Call { callee, args } => {
                let mut es: Vec<&Expr> = Vec::new();
                if let Callee::Indirect(c) = callee {
                    es.push(c);
                }
                es.extend(args.iter());
                let mut vs = self.values_in_order(&es);
                let f = match callee {
                    Callee::Direct(f) => fn_name(&self.m.functions[*f].name),
                    Callee::Indirect(_) => vs.remove(0),
                };
                format!("{f}({})", vs.join(", "))
            }
            Expr::Builtin { op, ty_args, args } => {
                let info = op.info();
                assert_eq!(args.len(), info.params.len(), "{op:?} の引数の数が違う");
                assert_eq!(
                    ty_args.len(),
                    info.ty_params as usize,
                    "{op:?} の型引数の数が違う"
                );
                let es: Vec<&Expr> = args.iter().collect();
                let mut vs = self.values_in_order(&es);
                if *op == Builtin::Dbg {
                    vs.push(self.show_fn(&ty_args[0]));
                }
                let f = self.rt(info.js);
                format!("{f}({})", vs.join(", "))
            }
            Expr::Ctor { ctor, args } => {
                let def = &self.m.enums[ctor.enum_id];
                let variant = &def.variants[ctor.variant];
                assert_eq!(
                    args.len(),
                    variant.fields.len(),
                    "{} の引数の数が違う",
                    variant.name
                );
                if variant.fields.is_empty() {
                    return unit_variant_name(&def.name, &variant.name);
                }
                let es: Vec<&Expr> = args.iter().collect();
                let vs = self.values_in_order(&es);
                let mut s = format!("{{ $tag: {}", ctor.variant);
                for (i, v) in vs.iter().enumerate() {
                    let _ = write!(s, ", {}: {v}", field_key(&variant.fields, i));
                }
                s.push_str(" }");
                s
            }
            Expr::Field { base, ctor, index } => {
                let b = self.value(base);
                let fields = &self.m.variant(*ctor).fields;
                format!("{b}{}", field_access(fields, *index))
            }
            Expr::Tuple(items) => {
                let es: Vec<&Expr> = items.iter().collect();
                let vs = self.values_in_order(&es);
                format!("[{}]", vs.join(", "))
            }
            Expr::List { elems, tail } => {
                let mut es: Vec<&Expr> = elems.iter().collect();
                es.extend(tail.as_deref());
                let mut vs = self.values_in_order(&es);
                let tail = tail.as_ref().map(|_| vs.pop().unwrap());
                if vs.is_empty() {
                    return tail.unwrap_or_else(|| "null".to_owned());
                }
                let list = self.rt("$list");
                match tail {
                    Some(t) => format!("{list}([{}], {t})", vs.join(", ")),
                    None => format!("{list}([{}])", vs.join(", ")),
                }
            }
            Expr::If { cond, then, else_ } if is_simple(e) => {
                let c = self.value(cond);
                let a = self.value(then);
                let b = self.value(else_);
                format!("({c} ? {a} : {b})")
            }
            Expr::Binary { op, ty, lhs, rhs } => self.binary(*op, *ty, lhs, rhs),
            Expr::Unary { op, ty, operand } => {
                let v = self.value(operand);
                match (op, ty) {
                    (UnOp::Neg, OpTy::Int) => format!("((-{v}) | 0)"),
                    (UnOp::Neg, OpTy::Int64) => format!("BigInt.asIntN(64, -{v})"),
                    (UnOp::Neg, OpTy::Float) => format!("(-{v})"),
                    (UnOp::Not, OpTy::Bool) => format!("(!{v})"),
                    _ => panic!("不正な IR: 単項演算 {op:?} は {ty:?} に使えない"),
                }
            }
            Expr::Lambda { params, body } => self.lambda(params, body),
            Expr::Concat(parts) => {
                let es: Vec<&Expr> = parts
                    .iter()
                    .filter_map(|p| match p {
                        StrPart::Value(v, _) => Some(v),
                        StrPart::Lit(_) => None,
                    })
                    .collect();
                let mut vs = self.values_in_order(&es).into_iter();
                let mut s = String::from("`");
                for p in parts {
                    match p {
                        StrPart::Lit(text) => s.push_str(&template_escape(text)),
                        StrPart::Value(_, ty) => {
                            let v = vs.next().unwrap();
                            // 補間では String をそのまま埋め込む．Int と Int64 はテンプレートの
                            // 変換で `String` と同じ文字列になる．それ以外は表示関数を通す．
                            let shown = match ty {
                                Type::Int | Type::Int64 | Type::String => v,
                                _ => self.show_call(ty, &v),
                            };
                            let _ = write!(s, "${{{shown}}}");
                        }
                    }
                }
                s.push('`');
                s
            }
            Expr::If { .. } | Expr::Match { .. } | Expr::Loop { .. } => self.via_tmp(e),
            Expr::Recur(_) => panic!("不正な IR: Recur が Loop の末尾位置の外にある"),
        }
    }

    /// 型 `ty` の値 `v`（JS の式）を表示した文字列の JS の式．
    ///
    /// `Type::Param(i)` は enum の表示関数の中でだけ使え，引数 `$a{i}` で受けた表示関数を呼ぶ．
    fn show_call(&mut self, ty: &Type, v: &str) -> String {
        match ty {
            Type::List(elem) => {
                let f = self.show_fn(elem);
                format!("{}({v}, {f})", self.rt("$showList"))
            }
            Type::Tuple(items) => {
                let fs: Vec<String> = items.iter().map(|t| self.show_fn(t)).collect();
                format!("{}({v}, [{}])", self.rt("$showTuple"), fs.join(", "))
            }
            Type::Enum(id, args) => {
                let mut s = format!("{}({v}", self.show_enum(*id));
                for a in args {
                    let f = self.show_fn(a);
                    let _ = write!(s, ", {f}");
                }
                s.push(')');
                s
            }
            _ => format!("{}({v})", self.show_fn(ty)),
        }
    }

    /// 型 `ty` の値を表示する関数の JS の式．
    fn show_fn(&mut self, ty: &Type) -> String {
        match ty {
            Type::Int => self.rt("$showInt").to_owned(),
            Type::Int64 => self.rt("$showInt64").to_owned(),
            Type::Float => self.rt("$showFloat").to_owned(),
            Type::Bool => self.rt("$showBool").to_owned(),
            Type::String => self.rt("$showString").to_owned(),
            Type::Unit => self.rt("$showUnit").to_owned(),
            // Never の値は作れないので呼ばれない．
            Type::Never => self.rt("$unreachable").to_owned(),
            Type::Param(i) => format!("$a{i}"),
            Type::Enum(id, args) if args.is_empty() => self.show_enum(*id),
            Type::List(_) | Type::Tuple(_) | Type::Enum(..) => {
                let body = self.show_call(ty, "$x");
                format!("(($x) => {body})")
            }
            Type::Fn => panic!("不正な IR: 関数値は表示できない（仕様 10.6）"),
        }
    }

    /// enum の表示関数の名前．まだ出していなければ出す予定に入れる．
    fn show_enum(&mut self, id: EnumId) -> String {
        if !self.shows.contains(&id) {
            self.shows.push(id);
        }
        format!("$show${}", sanitize(&self.m.enums[id].name))
    }

    /// enum の表示関数を出す．型引数の表示関数を `$a0`，`$a1`，… で受ける．
    /// 構成子はソースの書き方で表す（`Circle(radius: 1.0)`，`Rect(1.0, 2.0)`，`Empty`）．
    fn show_enum_fn(&mut self, id: EnumId) {
        let m = self.m;
        let def = &m.enums[id];
        let mut params = vec!["v".to_owned()];
        params.extend((0..def.params).map(|i| format!("$a{i}")));
        let name = self.show_enum(id);
        self.line(&format!("function {name}({}) {{", params.join(", ")));
        self.indent += 1;
        let switch = def.variants.len() > 1;
        if switch {
            self.line("switch (v.$tag) {");
            self.indent += 1;
        }
        for (k, variant) in def.variants.iter().enumerate() {
            assert_eq!(
                variant.tys.len(),
                variant.fields.len(),
                "不正な IR: {} のフィールドの型の数が違う",
                variant.name
            );
            let mut pieces = Pieces::default();
            pieces.text(&variant.name);
            for (i, ty) in variant.tys.iter().enumerate() {
                pieces.text(if i == 0 { "(" } else { ", " });
                if let VariantFields::Named(names) = &variant.fields {
                    pieces.text(&format!("{}: ", names[i]));
                }
                let access = format!("v{}", field_access(&variant.fields, i));
                pieces.js(self.show_call(ty, &access));
            }
            if !variant.tys.is_empty() {
                pieces.text(")");
            }
            let expr = pieces.concat();
            if switch {
                self.line(&format!("case {k}: return {expr};"));
            } else {
                self.line(&format!("return {expr};"));
            }
        }
        if switch {
            self.indent -= 1;
            self.line("}");
            let u = self.rt("$unreachable");
            self.line(&format!("return {u}();"));
        }
        self.indent -= 1;
        self.line("}");
    }

    fn binary(&mut self, op: BinOp, ty: OpTy, lhs: &Expr, rhs: &Expr) -> String {
        if matches!(op, BinOp::And | BinOp::Or) {
            assert_eq!(ty, OpTy::Bool, "不正な IR: {op:?} は Bool にだけ使える");
            let js_op = if op == BinOp::And { "&&" } else { "||" };
            if is_simple(rhs) {
                let a = self.value(lhs);
                let b = self.value(rhs);
                return format!("({a} {js_op} {b})");
            }
            // 右辺が文を要するときは，左辺の結果で分岐してから右辺を評価する．
            let a = self.value(lhs);
            let t = self.tmp();
            self.line(&format!("let {t} = {a};"));
            let test = if op == BinOp::And {
                t.clone()
            } else {
                format!("!{t}")
            };
            self.line(&format!("if ({test}) {{"));
            self.indent += 1;
            self.stmt(rhs, &Dest::Assign(t.clone()));
            self.indent -= 1;
            self.line("}");
            return t;
        }

        let vs = self.values_in_order(&[lhs, rhs]);
        let (a, b) = (&vs[0], &vs[1]);
        let cmp = |js: &str| format!("({a} {js} {b})");
        match (ty, op) {
            (OpTy::Int, BinOp::Add) => format!("(({a} + {b}) | 0)"),
            (OpTy::Int, BinOp::Sub) => format!("(({a} - {b}) | 0)"),
            (OpTy::Int, BinOp::Mul) => format!("Math.imul({a}, {b})"),
            (OpTy::Int, BinOp::Div) => format!("{}({a}, {b})", self.rt("$idiv")),
            (OpTy::Int, BinOp::Rem) => format!("{}({a}, {b})", self.rt("$irem")),
            (OpTy::Int64, BinOp::Add) => format!("BigInt.asIntN(64, {a} + {b})"),
            (OpTy::Int64, BinOp::Sub) => format!("BigInt.asIntN(64, {a} - {b})"),
            (OpTy::Int64, BinOp::Mul) => format!("BigInt.asIntN(64, {a} * {b})"),
            (OpTy::Int64, BinOp::Div) => format!("{}({a}, {b})", self.rt("$ldiv")),
            (OpTy::Int64, BinOp::Rem) => format!("{}({a}, {b})", self.rt("$lrem")),
            (OpTy::Float, BinOp::Add) => cmp("+"),
            (OpTy::Float, BinOp::Sub) => cmp("-"),
            (OpTy::Float, BinOp::Mul) => cmp("*"),
            (OpTy::Float, BinOp::Div) => cmp("/"),
            (OpTy::Float, BinOp::Rem) => cmp("%"),
            (OpTy::Structural, BinOp::Eq) => format!("{}({a}, {b})", self.rt("$eq")),
            (OpTy::Structural, BinOp::Ne) => format!("(!{}({a}, {b}))", self.rt("$eq")),
            (OpTy::String, BinOp::Eq) => cmp("==="),
            (OpTy::String, BinOp::Ne) => cmp("!=="),
            (OpTy::String, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) => {
                format!("({}({a}, {b}) {} 0)", self.rt("$scmp"), cmp_op(op))
            }
            (
                OpTy::Int | OpTy::Int64 | OpTy::Float | OpTy::Bool,
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
            ) => cmp(cmp_op(op)),
            _ => panic!("不正な IR: 二項演算 {op:?} は {ty:?} に使えない"),
        }
    }

    fn lambda(&mut self, params: &[Local], body: &Expr) -> String {
        let ps: Vec<String> = params.iter().map(|&p| self.local(p)).collect();
        let ps = ps.join(", ");
        // ラムダの本体は別の関数なので，囲むループへは続かない．
        let loops = std::mem::take(&mut self.loops);
        let s = if is_simple(body) {
            let v = self.value(body);
            format!("(({ps}) => ({v}))")
        } else {
            let outer = std::mem::take(&mut self.out);
            self.indent += 1;
            self.stmt(body, &Dest::Return);
            self.indent -= 1;
            let inner = std::mem::replace(&mut self.out, outer);
            let mut close = String::new();
            for _ in 0..self.indent {
                close.push_str("  ");
            }
            format!("(({ps}) => {{\n{inner}{close}}})")
        };
        self.loops = loops;
        s
    }

    fn stmt(&mut self, e: &Expr, dest: &Dest) {
        match e {
            Expr::Let { var, value, body } => {
                let v = self.value(value);
                let name = self.local(*var);
                self.line(&format!("const {name} = {v};"));
                self.stmt(body, dest);
            }
            Expr::If { cond, then, else_ } if !is_simple(e) => {
                let c = self.value(cond);
                self.line(&format!("if ({}) {{", unparen(&c)));
                self.indent += 1;
                self.stmt(then, dest);
                self.indent -= 1;
                self.line("} else {");
                self.indent += 1;
                self.stmt(else_, dest);
                self.indent -= 1;
                self.line("}");
            }
            Expr::Match { scrutinee, arms } => self.match_(scrutinee, arms, dest),
            Expr::Loop { vars, inits, body } => {
                let es: Vec<&Expr> = inits.iter().collect();
                let vs = self.values_in_order(&es);
                let label = self.label("L");
                let cells: Vec<String> = (0..vars.len()).map(|_| self.tmp()).collect();
                if !cells.is_empty() {
                    let decls: Vec<String> = cells
                        .iter()
                        .zip(&vs)
                        .map(|(c, v)| format!("{c} = {v}"))
                        .collect();
                    self.line(&format!("let {};", decls.join(", ")));
                }
                self.line(&format!("{label}: for (;;) {{"));
                self.indent += 1;
                for (&var, cell) in vars.iter().zip(&cells) {
                    let name = self.local(var);
                    self.line(&format!("const {name} = {cell};"));
                }
                self.loops.push(LoopCtx {
                    label: label.clone(),
                    cells,
                });
                self.stmt(body, dest);
                self.loops.pop();
                if let Dest::Assign(_) = dest {
                    self.line(&format!("break {label};"));
                }
                self.indent -= 1;
                self.line("}");
            }
            Expr::Recur(args) => {
                let es: Vec<&Expr> = args.iter().collect();
                let vs = self.values_in_order(&es);
                let ctx = self
                    .loops
                    .last()
                    .expect("不正な IR: Recur が Loop の外にある");
                assert_eq!(
                    ctx.cells.len(),
                    vs.len(),
                    "不正な IR: Recur の引数の数が違う"
                );
                let assigns: Vec<String> = ctx
                    .cells
                    .iter()
                    .zip(&vs)
                    .map(|(c, v)| format!("{c} = {v};"))
                    .collect();
                let label = ctx.label.clone();
                for a in assigns {
                    self.line(&a);
                }
                self.line(&format!("continue {label};"));
            }
            _ => {
                let v = self.value(e);
                self.emit_dest(dest, &v);
            }
        }
    }

    fn match_(&mut self, scrutinee: &Expr, arms: &[Arm], dest: &Dest) {
        let v = self.value(scrutinee);
        let s = if matches!(scrutinee, Expr::Var(_) | Expr::Lit(_)) {
            v
        } else {
            let t = self.tmp();
            self.line(&format!("const {t} = {v};"));
            t
        };

        // 行き先が return でなければ，腕の後で抜けるためにラベル付きのブロックで囲む．
        let label = match dest {
            Dest::Return => None,
            Dest::Assign(_) => {
                let l = self.label("m");
                self.line(&format!("{l}: {{"));
                self.indent += 1;
                Some(l)
            }
        };

        let mut exhaustive = false;
        for arm in arms {
            let mut conds = Vec::new();
            let mut binds = Vec::new();
            self.pattern(&arm.pat, s.clone(), &mut conds, &mut binds);
            let mut opened = 0;
            if !conds.is_empty() {
                self.line(&format!("if ({}) {{", conds.join(" && ")));
                self.indent += 1;
                opened += 1;
            }
            for (l, access) in binds {
                let name = self.local(l);
                self.line(&format!("const {name} = {access};"));
            }
            if let Some(g) = &arm.guard {
                let gv = self.value(g);
                self.line(&format!("if ({}) {{", unparen(&gv)));
                self.indent += 1;
                opened += 1;
            }
            self.stmt(&arm.body, dest);
            if let Some(l) = &label {
                self.line(&format!("break {l};"));
            }
            for _ in 0..opened {
                self.indent -= 1;
                self.line("}");
            }
            if opened == 0 {
                // 無条件に合う腕より後ろの腕には届かない．
                exhaustive = true;
                break;
            }
        }
        if !exhaustive {
            let u = self.rt("$unreachable");
            self.line(&format!("{u}();"));
        }

        if label.is_some() {
            self.indent -= 1;
            self.line("}");
        }
    }

    /// `access` の値が `p` に合う条件と，束縛する変数を集める．条件は左から順に評価してよい形に並ぶ．
    fn pattern(
        &self,
        p: &Pat,
        access: String,
        conds: &mut Vec<String>,
        binds: &mut Vec<(Local, String)>,
    ) {
        match p {
            Pat::Wild | Pat::Lit(Lit::Unit) => {}
            Pat::Bind(l) => binds.push((*l, access)),
            Pat::Lit(lit) => conds.push(format!("{access} === {}", lit_js(lit))),
            Pat::Ctor { ctor, fields } => {
                let def = &self.m.enums[ctor.enum_id];
                let variant = &def.variants[ctor.variant];
                assert_eq!(
                    fields.len(),
                    variant.fields.len(),
                    "{} のフィールドの数が違う",
                    variant.name
                );
                if def.variants.len() > 1 {
                    conds.push(format!("{access}.$tag === {}", ctor.variant));
                }
                for (i, f) in fields.iter().enumerate() {
                    let sub = format!("{access}{}", field_access(&variant.fields, i));
                    self.pattern(f, sub, conds, binds);
                }
            }
            Pat::Tuple(items) => {
                for (i, item) in items.iter().enumerate() {
                    self.pattern(item, format!("{access}[{i}]"), conds, binds);
                }
            }
            Pat::List { elems, rest } => {
                let mut cur = access;
                for elem in elems {
                    conds.push(format!("{cur} !== null"));
                    self.pattern(elem, format!("{cur}.h"), conds, binds);
                    cur = format!("{cur}.t");
                }
                match rest {
                    None => conds.push(format!("{cur} === null")),
                    Some(r) => self.pattern(r, cur, conds, binds),
                }
            }
        }
    }
}

/// 文字列の連結の部品．隣り合う文字列はまとめて1つのリテラルにする．
#[derive(Default)]
struct Pieces(Vec<(bool, String)>);

impl Pieces {
    fn text(&mut self, t: &str) {
        match self.0.last_mut() {
            Some((true, last)) => last.push_str(t),
            _ => self.0.push((true, t.to_owned())),
        }
    }

    fn js(&mut self, e: String) {
        self.0.push((false, e));
    }

    /// `+` でつないだ JS の式．
    fn concat(self) -> String {
        let parts: Vec<String> = self
            .0
            .into_iter()
            .map(|(is_text, s)| if is_text { js_string(&s) } else { s })
            .collect();
        parts.join(" + ")
    }
}

/// 文を出さずに JS の式1つにできるか．
fn is_simple(e: &Expr) -> bool {
    match e {
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn(_) | Expr::Lambda { .. } => true,
        Expr::Let { .. } | Expr::Match { .. } | Expr::Loop { .. } | Expr::Recur(_) => false,
        Expr::Call { callee, args } => {
            let callee_simple = match callee {
                Callee::Direct(_) => true,
                Callee::Indirect(c) => is_simple(c),
            };
            callee_simple && args.iter().all(is_simple)
        }
        Expr::Builtin { args, .. } | Expr::Ctor { args, .. } | Expr::Tuple(args) => {
            args.iter().all(is_simple)
        }
        Expr::Field { base, .. } => is_simple(base),
        Expr::List { elems, tail } => {
            elems.iter().all(is_simple) && tail.as_deref().is_none_or(is_simple)
        }
        Expr::If { cond, then, else_ } => is_simple(cond) && is_simple(then) && is_simple(else_),
        Expr::Binary { lhs, rhs, .. } => is_simple(lhs) && is_simple(rhs),
        Expr::Unary { operand, .. } => is_simple(operand),
        Expr::Concat(parts) => parts.iter().all(|p| match p {
            StrPart::Lit(_) => true,
            StrPart::Value(v, _) => is_simple(v),
        }),
    }
}

/// 評価しても何も起きず，何度評価しても同じ値の式．
fn is_atom(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit(_) | Expr::Var(_) | Expr::Fn(_) | Expr::Lambda { .. }
    )
}

/// 全体を囲む括弧が1組あれば外す（`if` の条件を読みやすくするため）．
/// 文字列を含むときは括弧を数え違えうるので外さない．
fn unparen(s: &str) -> &str {
    let Some(inner) = s.strip_prefix('(').and_then(|t| t.strip_suffix(')')) else {
        return s;
    };
    if inner.contains(['"', '`']) {
        return s;
    }
    let mut depth = 0i32;
    for c in inner.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return s;
                }
            }
            _ => {}
        }
    }
    if depth == 0 { inner } else { s }
}

fn cmp_op(op: BinOp) -> &'static str {
    match op {
        BinOp::Eq => "===",
        BinOp::Ne => "!==",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        _ => unreachable!(),
    }
}

fn lit_js(lit: &Lit) -> String {
    match lit {
        Lit::Int(n) if *n < 0 => format!("({n})"),
        Lit::Int(n) => n.to_string(),
        Lit::Int64(n) if *n < 0 => format!("({n}n)"),
        Lit::Int64(n) => format!("{n}n"),
        Lit::Float(x) if x.is_nan() => "NaN".to_owned(),
        Lit::Float(x) if x.is_infinite() => {
            if *x > 0.0 {
                "Infinity".to_owned()
            } else {
                "(-Infinity)".to_owned()
            }
        }
        Lit::Float(x) if x.is_sign_negative() => format!("({x:?})"),
        Lit::Float(x) => format!("{x:?}"),
        Lit::Bool(b) => b.to_string(),
        Lit::String(s) => js_string(s),
        Lit::Unit => "undefined".to_owned(),
    }
}

pub(crate) fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn template_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '`' => out.push_str("\\`"),
            '\\' => out.push_str("\\\\"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn field_key(fields: &VariantFields, i: usize) -> String {
    match fields {
        VariantFields::Named(names) if names[i] == "__proto__" => "[\"__proto__\"]".to_owned(),
        VariantFields::Named(names) => names[i].clone(),
        _ => format!("${i}"),
    }
}

fn field_access(fields: &VariantFields, i: usize) -> String {
    match fields {
        VariantFields::Named(names) if names[i] == "__proto__" => "[\"__proto__\"]".to_owned(),
        VariantFields::Named(names) => format!(".{}", names[i]),
        _ => format!(".${i}"),
    }
}

fn unit_variant_name(enum_name: &str, variant: &str) -> String {
    format!("{}${}", sanitize(enum_name), sanitize(variant))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// トップレベル関数の JS 名．予約語や生成コードが使う大域名と重なるときは `$` を付ける．
fn fn_name(name: &str) -> String {
    let s = sanitize(name);
    if RESERVED.contains(&s.as_str()) {
        format!("{s}$")
    } else {
        s
    }
}

const RESERVED: &[&str] = &[
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "arguments",
    "eval",
    "undefined",
    "NaN",
    "Infinity",
    "Math",
    "BigInt",
    "Object",
    "Intl",
    "Symbol",
    "Number",
    "String",
    "Array",
    "Error",
];
