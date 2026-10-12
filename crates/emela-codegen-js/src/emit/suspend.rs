//! 中断と再開（仕様 8.4，14.5，16.5）の出力．
//!
//! 中断しうる関数とラムダ（`Function::suspends`，`Expr::Lambda::suspends`）はジェネレータにし，
//! 中断しうる呼び出し（`Expr::Call::suspends`）を `yield*` でつなぐ．suspend fn の操作の呼び出しは
//! `yield $perform(h.op, [引数])` で，`@external suspend fn` は `yield $await(promise)` で，要求を
//! 最も外側のジェネレータを進めるランタイムの駆動（`$start`）まで送る．駆動が resume を作って
//! 操作の実装を呼び，resume された値で継続を進める．
//!
//! ```js
//! function* greet(ev) {
//!   const clock = ev.Clock;
//!   (yield $perform(clock.sleep, [1000]));   // suspend fn の操作
//!   return (yield* fetch_name());            // 中断しうる関数
//! }
//! ```
//!
//! 中断しない関数の出力は変わらない．失敗の `$Fail` はジェネレータの戻り値として同じように返り，
//! 自己末尾呼び出しのループ（16.3）は `Recur` が呼び出しを置き換えているので `yield*` を挟まない．

use emela_core::Callee;

use super::Emitter;

impl Emitter<'_> {
    /// 呼ばれる側 `f` と評価済みの引数 `args`（JS の式）の呼び出し．`suspends` は
    /// `Expr::Call::suspends`．
    pub(super) fn suspending_call(
        &mut self,
        callee: &Callee,
        f: &str,
        args: &[String],
        suspends: bool,
    ) -> String {
        let call = format!("{f}({})", args.join(", "));
        match callee {
            Callee::Direct(id) => {
                let target = &self.m.functions[*id];
                assert_eq!(
                    target.suspends, suspends,
                    "不正な IR: `{}` の呼び出しの suspends が関数の印と違う",
                    target.name
                );
                if !suspends {
                    return call;
                }
                self.expect_generator();
                format!("(yield* {call})")
            }
            Callee::Op { effect, op, .. } if self.m.effects[*effect].ops[*op].suspend => {
                let def = &self.m.effects[*effect];
                assert!(
                    suspends,
                    "不正な IR: suspend fn の操作 `{}.{}` の呼び出しが中断しないことになっている",
                    def.name, def.ops[*op].name
                );
                self.expect_generator();
                let perform = self.rt("$perform");
                format!("(yield {perform}({f}, [{}]))", args.join(", "))
            }
            // 関数の値と fn の操作は，実際の関数が中断しないものでもよい（`$maybe` が見分ける）．
            Callee::Indirect(_) | Callee::Op { .. } => {
                if !suspends {
                    return call;
                }
                self.expect_generator();
                let maybe = self.rt("$maybe");
                format!("(yield* {maybe}({call}))")
            }
        }
    }

    fn expect_generator(&self) {
        assert!(
            self.generator,
            "不正な IR: 中断しうる呼び出しが中断しない関数の中にある"
        );
    }
}
