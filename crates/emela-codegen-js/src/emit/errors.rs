//! エラー（仕様 7章）の出力．
//!
//! 失敗は戻り値として返す（16.5）．`fail e` はランタイムの `$Fail` に包んだエラーの値を返し，
//! 失敗しうる呼び出し（`Expr::Call::fails`）の結果が `$Fail` なら，それを失敗の行き先へ渡す．
//! 行き先は最も内側の escape か，関数の呼び出し元．
//!
//! escape は2重のラベル付きブロックにする．
//!
//! ```js
//! let $t0, $t1;          // escape の値，受けた失敗
//! $x0: {
//!   $e1: {
//!     ...                // 対象の式．失敗したら `{ $t1 = 失敗; break $e1; }`
//!     break $x0;
//!   }
//!   const $t2 = $t1.error;
//!   if ($t2.$tag === $err$NotFound) { ...; $t0 = 腕の値; break $x0; }
//!   return $t1;          // どの腕にも合わなかった失敗は外側へ伝える
//! }
//! ```
//!
//! error の値は `$tag` に error ごとの印（`$err$NotFound`）を持つ．印は表示関数も持ち，
//! 起動用モジュールとテストランナーが，処理されなかったエラーを表示するのに使う．

use emela_core::{Arm, EnumId, Expr, Pat, Type};

use super::{Dest, Emitter, sanitize, unparen};

/// 失敗の行き先．
pub(super) enum FailTarget {
    /// escape の対象の式の中．失敗を `cell` に入れて `label` のブロックを抜ける．
    Escape { label: String, cell: String },
}

impl Emitter<'_> {
    /// enum の値の `$tag` に書く JS の式．error なら印，それ以外はバリアントの添字．
    pub(super) fn tag(&self, id: EnumId, variant: usize) -> String {
        if self.m.enums[id].error {
            descriptor_name(&self.m.enums[id].name)
        } else {
            variant.to_string()
        }
    }

    /// error ごとの印をモジュールの先頭に出す．印は値の表示関数（`show`）を持つ．
    ///
    /// 関数型のフィールドを持つ error は表示できない（10.6）ので，error の名前だけを出す．
    pub(super) fn error_descriptors(&mut self) {
        let m = self.m;
        let mut any = false;
        for (id, def) in m.enums.iter() {
            if !def.error {
                continue;
            }
            let showable = def.variants.iter().all(|v| !v.tys.iter().any(contains_fn));
            let show = if showable {
                self.show_enum(id)
            } else {
                format!("() => {}", super::js_string(&def.name))
            };
            self.line(&format!(
                "const {} = Object.freeze({{ show: {show} }});",
                descriptor_name(&def.name)
            ));
            any = true;
        }
        if any {
            self.line("");
        }
    }

    /// 失敗 `v`（`$Fail` の JS の式）を行き先へ渡す文．
    fn propagate_stmt(&self, v: &str) -> String {
        match self.fail_targets.last() {
            None => format!("return {v};"),
            Some(FailTarget::Escape { label, cell }) => {
                format!("{{ {cell} = {v}; break {label}; }}")
            }
        }
    }

    /// 失敗しうる呼び出し `call` の結果を一時変数に入れ，失敗なら行き先へ渡す．値は一時変数の名前．
    pub(super) fn checked(&mut self, call: String) -> String {
        let t = self.tmp();
        self.line(&format!("const {t} = {call};"));
        let fail = self.rt("$Fail");
        let propagate = self.propagate_stmt(&t);
        self.line(&format!("if ({t} instanceof {fail}) {propagate}"));
        t
    }

    /// `fail e` の文．
    pub(super) fn fail(&mut self, e: &Expr) {
        let v = self.value(e);
        let fail = self.rt("$Fail");
        let stmt = self.propagate_stmt(&format!("new {fail}({v})"));
        self.line(&stmt);
    }

    /// `body escape { arms }` の値を `dest` へ渡す文．
    pub(super) fn escape(&mut self, body: &Expr, arms: &[Arm], dest: &Dest) {
        // 対象の式の中では呼び出し元へ直接返せないので，値はいつも一時変数で受ける．
        let result = match dest {
            Dest::Assign(t) => t.clone(),
            Dest::Return => {
                let t = self.tmp();
                self.line(&format!("let {t};"));
                t
            }
        };
        let cell = self.tmp();
        self.line(&format!("let {cell};"));
        let done = self.label("x");
        let caught = self.label("e");
        self.line(&format!("{done}: {{"));
        self.indent += 1;

        self.line(&format!("{caught}: {{"));
        self.indent += 1;
        self.fail_targets.push(FailTarget::Escape {
            label: caught,
            cell: cell.clone(),
        });
        self.stmt(body, &Dest::Assign(result.clone()));
        self.fail_targets.pop();
        self.line(&format!("break {done};"));
        self.indent -= 1;
        self.line("}");

        // 腕は対象の式の外なので，腕の中の失敗は外側の行き先へ伝わる．
        let error = self.tmp();
        self.line(&format!("const {error} = {cell}.error;"));
        // 上の腕が全部受けている error．その下の同じ error の腕には来ない．
        let mut total: Vec<EnumId> = Vec::new();
        for arm in arms {
            let Pat::Ctor { ctor, .. } = &arm.pat else {
                panic!("不正な IR: escape の腕のパターンが error の構成子でない");
            };
            assert!(
                self.m.enums[ctor.enum_id].error,
                "不正な IR: escape の腕のパターンが error の構成子でない"
            );
            if total.contains(&ctor.enum_id) {
                continue;
            }
            let mut conds = vec![format!(
                "{error}.$tag === {}",
                self.tag(ctor.enum_id, ctor.variant)
            )];
            let mut binds = Vec::new();
            self.pattern(&arm.pat, error.clone(), &mut conds, &mut binds);
            if conds.len() == 1 && arm.guard.is_none() {
                total.push(ctor.enum_id);
            }
            self.line(&format!("if ({}) {{", conds.join(" && ")));
            self.indent += 1;
            for (l, access) in binds {
                let name = self.local(l);
                self.line(&format!("const {name} = {access};"));
            }
            let mut opened = 0;
            if let Some(g) = &arm.guard {
                let gv = self.value(g);
                self.line(&format!("if ({}) {{", unparen(&gv)));
                self.indent += 1;
                opened += 1;
            }
            self.stmt(&arm.body, &Dest::Assign(result.clone()));
            self.line(&format!("break {done};"));
            for _ in 0..opened {
                self.indent -= 1;
                self.line("}");
            }
            self.indent -= 1;
            self.line("}");
        }
        // どの腕にも合わなかった失敗（腕に書かれていない error と，一部の値しか受けない腕に
        // 合わなかった値）は外側へ伝える（7.3）．
        let propagate = self.propagate_stmt(&cell);
        self.line(&propagate);

        self.indent -= 1;
        self.line("}");
        if let Dest::Return = dest {
            self.line(&format!("return {result};"));
        }
    }
}

/// error の印の JS の名前．
fn descriptor_name(error: &str) -> String {
    format!("$err${}", sanitize(error))
}

/// 関数型を含む型か（表示できない，10.6）．error は型引数を持たないので `Type::Param` は来ない．
fn contains_fn(ty: &Type) -> bool {
    match ty {
        Type::Fn => true,
        Type::Tuple(ts) | Type::Enum(_, ts) => ts.iter().any(contains_fn),
        Type::List(t) => contains_fn(t),
        _ => false,
    }
}
