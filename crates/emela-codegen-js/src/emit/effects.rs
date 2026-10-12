//! エフェクト（仕様 8章）と外部関数（12章）の出力．
//!
//! ハンドラは引数で渡して直接呼ぶ（16.5）．証拠の表は，エフェクトの名前を鍵にハンドラの値を
//! 持つオブジェクトで，R が空でない関数とラムダの最後の引数に渡す．with は表を `__proto__` で
//! 延ばした新しいオブジェクトを作る（外側の表は変えない）．ハンドラの値は操作の名前を鍵に
//! 関数を持つオブジェクト．
//!
//! 外部関数は，ホストの関数を呼ぶ包みの関数にする．投げた例外は defect にし（12.5），
//! ホストから受け取る文字列の対になっていないサロゲートを置換文字に直す（12.5）．
//! `@external suspend fn` の包みはジェネレータで，ホストの返した Promise を待つ（12.2）．

use emela_core::{Boundary, EffectId, Expr, External, Function, Module, Release, WithBind};

use super::{Dest, Emitter, fn_name, function_keyword, is_identifier_name, js_string, sanitize};

/// 証拠の表の鍵．
pub(crate) fn effect_key(m: &Module, effect: EffectId) -> String {
    sanitize(&m.effects[effect].name)
}

/// ハンドラの値の操作の参照（`.print_line`）．
pub(crate) fn op_access(name: &str) -> String {
    if name == "__proto__" {
        "[\"__proto__\"]".to_owned()
    } else {
        format!(".{name}")
    }
}

/// ハンドラの値のオブジェクトリテラルの鍵．
fn op_key(name: &str) -> String {
    if name == "__proto__" {
        "[\"__proto__\"]".to_owned()
    } else {
        name.to_owned()
    }
}

/// import の指定．絶対パス（lowering がソースの隣のファイルに直したもの）は `file:` の URL にする．
/// それ以外（`node:fs`，パッケージ名）は書かれたまま．
pub(crate) fn module_specifier(module: &str) -> String {
    if !module.starts_with('/') {
        return module.to_owned();
    }
    let mut url = String::from("file://");
    for b in module.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

impl Emitter<'_> {
    /// `{ __proto__: base, Console: h }`．
    pub(super) fn provide(&mut self, base: Option<&Expr>, bindings: &[(EffectId, Expr)]) -> String {
        let mut es: Vec<&Expr> = base.into_iter().collect();
        es.extend(bindings.iter().map(|(_, h)| h));
        let mut vs = self.values_in_order(&es);
        let base = match base {
            Some(_) => vs.remove(0),
            None => "null".to_owned(),
        };
        let mut s = format!("{{ __proto__: {base}");
        for ((effect, _), v) in bindings.iter().zip(vs) {
            s.push_str(&format!(", {}: {v}", effect_key(self.m, *effect)));
        }
        s.push_str(" }");
        s
    }

    /// `{ print_line: (text) => …, … }`．suspend fn の操作は `$suspendOp` でハンドラと操作の名前を
    /// 付ける（2回目の resume の defect の文面に使う，14.5）．
    pub(super) fn handler(&mut self, handler: &str, effect: EffectId, ops: &[Expr]) -> String {
        let m = self.m;
        let names = &m.effects[effect].ops;
        assert_eq!(
            names.len(),
            ops.len(),
            "不正な IR: ハンドラの操作の数が違う"
        );
        let es: Vec<&Expr> = ops.iter().collect();
        let vs = self.values_in_order(&es);
        let fields: Vec<String> = names
            .iter()
            .zip(vs)
            .map(|(op, v)| {
                let v = if op.suspend {
                    format!(
                        "{}({}, {}, {v})",
                        self.rt("$suspendOp"),
                        js_string(handler),
                        js_string(&op.name)
                    )
                } else {
                    v
                };
                format!("{}: {v}", op_key(&op.name))
            })
            .collect();
        format!("{{ {} }}", fields.join(", "))
    }

    /// release のない with の束縛（ハンドラの値と証拠の表）．
    pub(super) fn with_binds(&mut self, binds: &[WithBind]) {
        for b in binds {
            assert!(b.release.is_none(), "release のある束縛は with_ で出す");
            self.bind(b);
        }
    }

    fn bind(&mut self, b: &WithBind) -> String {
        let v = self.value(&b.value);
        let name = self.local(b.var);
        self.line(&format!("const {name} = {v};"));
        name
    }

    /// `with` の値を `dest` へ渡す文（14.2，14.4）．
    ///
    /// release のある束縛ごとに，それより後ろの束縛と本体を `try` で囲み，`finally` で release を
    /// 評価する．後ろの束縛（init）の失敗も本体の失敗も `return` か `break` で `try` を抜けるので，
    /// 束縛と逆の順に release が走る．中断されたまま巻き戻るときは，ランタイムが `gen.return()` で
    /// ジェネレータを閉じて同じ `finally` を走らせる（`$sync` の `$Interrupted` は例外として抜ける）．
    ///
    /// ```js
    /// const h$1 = Pool();
    /// let $t0;                         // 本体が投げたもの（defect か $Interrupted）
    /// try {
    ///   …                              // 後ろの束縛と本体
    /// } catch ($e) {
    ///   $t0 = $e;
    ///   throw $e;
    /// } finally {
    ///   try {
    ///     …                            // release
    ///   } catch ($e) {
    ///     throw $releaseDefect($t0, $e, "Pool");
    ///   } finally {
    ///     $retire(h$1, "Pool");
    ///   }
    /// }
    /// ```
    pub(super) fn with_(&mut self, binds: &[WithBind], body: &Expr, dest: &Dest) {
        let mut open: Vec<(String, String, &Release)> = Vec::new();
        for b in binds {
            let name = self.bind(b);
            if let Some(r) = &b.release {
                let thrown = self.tmp();
                self.line(&format!("let {thrown};"));
                self.line("try {");
                self.indent += 1;
                open.push((name, thrown, r));
            }
        }
        self.stmt(body, dest);
        for (name, thrown, r) in open.into_iter().rev() {
            self.indent -= 1;
            self.line("} catch ($e) {");
            self.line(&format!("  {thrown} = $e;"));
            self.line("  throw $e;");
            self.line("} finally {");
            self.indent += 1;
            self.line("try {");
            self.indent += 1;
            // release の値は捨てる．失敗しない（E が空，8.4）ので行き先も要らない．
            let v = self.value(&r.body);
            if !super::is_atom(&r.body) {
                self.line(&format!("{v};"));
            }
            self.indent -= 1;
            let handler = js_string(&r.handler);
            let merge = self.rt("$releaseDefect");
            self.line("} catch ($e) {");
            self.line(&format!("  throw {merge}({thrown}, $e, {handler});"));
            let retire = self.rt("$retire");
            self.line("} finally {");
            self.line(&format!("  {retire}({name}, {handler});"));
            self.line("}");
            self.indent -= 1;
            self.line("}");
        }
    }

    /// 外部関数の包み．JS の `@external` がなければ，呼ばれたら defect にする（エントリから
    /// たどれる関数は lowering が誤りにしてあるので，ここに来るのはたどれない関数だけ）．
    ///
    /// `@external suspend fn`（`Function::suspends`）の包みはジェネレータで，ホストの関数の返した値
    /// （Promise）を `yield $await(…)` で待ち，解決した値を戻り値と同じ規則で写す．拒否されたら，
    /// 駆動がその理由を `$foreign` で defect にして待っていた位置へ投げる（12.5）．
    pub(super) fn external_fn(&mut self, f: &Function, ext: &External) {
        let params: Vec<String> = f.params.iter().map(|&p| self.local(p)).collect();
        self.line(&format!(
            "{} {}({}) {{",
            function_keyword(f.suspends),
            fn_name(&f.name),
            params.join(", ")
        ));
        self.indent += 1;
        let callee = ext.binding("js").and_then(|b| match b.args.as_slice() {
            [path] => Some(global_ref(path)),
            [module, name] => Some(self.import(module, name)),
            _ => None,
        });
        match callee {
            Some(callee) => {
                let args: Vec<String> = params
                    .iter()
                    .zip(&ext.params)
                    .map(|(p, b)| self.convert(b, p, Dir::ToHost, 0))
                    .collect();
                let call = format!("{callee}({})", args.join(", "));
                let foreign = self.rt("$foreign");
                if f.suspends {
                    self.suspending_external(&call, &ext.ret, foreign);
                    self.indent -= 1;
                    self.line("}");
                    return;
                }
                self.line("try {");
                self.indent += 1;
                match &ext.ret {
                    Boundary::Unit => {
                        self.line(&format!("{call};"));
                        self.line("return undefined;");
                    }
                    ret => {
                        let v = self.convert(ret, &call, Dir::FromHost, 0);
                        self.line(&format!("return {v};"));
                    }
                }
                self.indent -= 1;
                self.line("} catch ($e) {");
                self.line(&format!("  throw {foreign}($e);"));
                self.line("}");
            }
            None => {
                let no = self.rt("$noExternal");
                self.line(&format!("return {no}({});", js_string(&f.name)));
            }
        }
        self.indent -= 1;
        self.line("}");
    }

    /// `@external suspend fn` の包みの本体．`call` はホストの関数の呼び出し．
    fn suspending_external(&mut self, call: &str, ret: &Boundary, foreign: &str) {
        self.line("let $p;");
        self.line("try {");
        self.line(&format!("  $p = {call};"));
        self.line("} catch ($e) {");
        self.line(&format!("  throw {foreign}($e);"));
        self.line("}");
        let wait = format!("(yield {}($p))", self.rt("$await"));
        match ret {
            Boundary::Unit => {
                self.line(&format!("{wait};"));
                self.line("return undefined;");
            }
            ret => {
                self.line(&format!("const $v = {wait};"));
                let v = self.convert(ret, "$v", Dir::FromHost, 0);
                self.line(&format!("return {v};"));
            }
        }
    }

    /// モジュールの名前を import して，局所名を返す．同じものは1回だけ import する．
    fn import(&mut self, module: &str, name: &str) -> String {
        if let Some((_, _, local)) = self
            .imports
            .iter()
            .find(|(m, n, _)| m == module && n == name)
        {
            return local.clone();
        }
        let local = format!("$ext{}", self.imports.len());
        self.imports
            .push((module.to_owned(), name.to_owned(), local.clone()));
        local
    }

    /// 値 `v`（JS の式．1回だけ使う）を境界の向き `dir` に写した JS の式．
    fn convert(&mut self, b: &Boundary, v: &str, dir: Dir, depth: usize) -> String {
        match (b, dir) {
            (Boundary::String, Dir::FromHost) => format!("{}({v})", self.rt("$wellFormed")),
            (Boundary::Fn { params, ret }, _) => {
                // コールバックの引数は逆の向きに，戻り値は同じ向きに写す．
                let names: Vec<String> = (0..params.len())
                    .map(|i| format!("$a{depth}_{i}"))
                    .collect();
                let args: Vec<String> = params
                    .iter()
                    .zip(&names)
                    .map(|(p, n)| self.convert(p, n, dir.flip(), depth + 1))
                    .collect();
                let call = format!("$f{depth}({})", args.join(", "));
                let body = self.convert(ret, &call, dir, depth + 1);
                format!("(($f{depth}) => ({}) => {body})({v})", names.join(", "))
            }
            _ => v.to_owned(),
        }
    }
}

/// 値の渡る向き．
#[derive(Clone, Copy)]
enum Dir {
    /// Emela からホストへ．
    ToHost,
    /// ホストから Emela へ．
    FromHost,
}

impl Dir {
    fn flip(self) -> Dir {
        match self {
            Dir::ToHost => Dir::FromHost,
            Dir::FromHost => Dir::ToHost,
        }
    }
}

/// グローバルの参照（`console.log`）．識別子でない区切りは `globalThis["…"]` の添字にする．
fn global_ref(path: &str) -> String {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.iter().all(|p| is_identifier_name(p)) {
        return path.to_owned();
    }
    let mut s = String::from("globalThis");
    for p in parts {
        s.push_str(&format!("[{}]", js_string(p)));
    }
    s
}
