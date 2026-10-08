//! match の網羅性と冗長な腕の検査．
//!
//! Maranget の usefulness（"Warnings for pattern matching", 2007）に基づく．
//! 「パターンの行列 P に対してベクトル q が有用（useful）か」，つまり q に合い P のどの行にも
//! 合わない値があるかを求め，その値の例（witness）を組み立てる．
//!
//! - 網羅性: ガードのない腕を並べた行列に対して `_` が有用なら，足りない値がある．
//! - 冗長性: 腕 i のパターンが，それより上のガードのない腕の行列に対して有用でなければ冗長．
//!
//! 構文木にも型推論にも依存しない．入力はこの検査専用の [`Pat`] と，型から構成子の形を引く
//! [`CtorTable`] で，型推論側から変換して呼ぶ．型の中身（構成子のフィールドの型）は特殊化の
//! たびに1段だけ引くので，再帰的な型でも止まる．

use std::fmt;

use smol_str::SmolStr;

/// 網羅性の検査に使うパターン．名前の解決と型検査は済んでいる前提．
#[derive(Clone, Debug, PartialEq)]
pub enum Pat {
    /// `_` と変数 `x`．
    Wild,
    /// リテラル `0` `1.5` `"a"` と定数 `MAX_SIZE`（定数は値に置き換えて渡す）．
    Lit(Lit),
    /// enum の構成子と `type`．`variant` は [`TyShape::Enum`] の何番目の構成子か．
    /// `fields` は（フィールドの宣言順の位置，パターン）の組で，書いていないフィールドは
    /// ワイルドカードとして扱う（`User(name:, ..)`）．
    Variant {
        variant: usize,
        fields: Vec<(usize, Pat)>,
    },
    /// タプル `(a, b)` と `()`．
    Tuple(Vec<Pat>),
    /// リスト．`[x, y]` は `rest: false`，`[x, ..rest]` は `rest: true`．
    List { prefix: Vec<Pat>, rest: bool },
}

impl Pat {
    /// 全フィールドを位置で並べた構成子パターン `Rect(w, h)`．
    pub fn variant(variant: usize, args: Vec<Pat>) -> Pat {
        Pat::Variant {
            variant,
            fields: args.into_iter().enumerate().collect(),
        }
    }

    /// 長さ固定のリスト `[x, y]`．
    pub fn list(elems: Vec<Pat>) -> Pat {
        Pat::List {
            prefix: elems,
            rest: false,
        }
    }

    /// 長さ n 以上のリスト `[x, ..rest]`．
    pub fn list_rest(prefix: Vec<Pat>) -> Pat {
        Pat::List { prefix, rest: true }
    }
}

/// リテラルの値．値の種類は無限なので，リテラルだけでは網羅できない．
#[derive(Clone, Debug, PartialEq)]
pub enum Lit {
    Int(i64),
    /// 値で比べる（`0.0` と `-0.0` は同じ）．
    Float(f64),
    Str(SmolStr),
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lit::Int(n) => write!(f, "{n}"),
            Lit::Float(x) => write!(f, "{x:?}"),
            Lit::Str(s) => write!(f, "{s:?}"),
        }
    }
}

/// 型が値をどう作るか．[`CtorTable::shape`] が返す．
#[derive(Clone, Debug)]
pub enum TyShape<T> {
    /// 構成子を数え上げられない型（`Int`，`Float`，`String`，関数など）．
    /// リテラルで照合できるが，ワイルドカードなしでは網羅できない．
    Opaque,
    /// enum．`Bool`（`True` / `False`），`Option`，構成子1つの `type` もこれで表す．
    Enum(Vec<VariantShape<T>>),
    /// タプル．`()` は要素なし．
    Tuple(Vec<T>),
    /// 要素の型が `T` のリスト．
    List(T),
}

/// enum の構成子1つ．`fields` は宣言順のフィールドの型（型引数は具体化済み）．
#[derive(Clone, Debug)]
pub struct VariantShape<T> {
    pub name: SmolStr,
    pub fields: Vec<T>,
}

/// 型から構成子の形を引く表．型推論側が実装する．
pub trait CtorTable {
    type Ty: Clone;

    fn shape(&self, ty: &Self::Ty) -> TyShape<Self::Ty>;
}

/// match の腕1つ．
#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pat,
    /// `if 条件` が付いているか．
    pub guarded: bool,
}

/// 検査の結果．
#[derive(Clone, Debug, PartialEq)]
pub struct MatchCheck {
    /// 足りない値の例．空なら網羅している．多すぎるときは [`WITNESS_LIMIT`] 個で打ち切る．
    pub missing: Vec<Witness>,
    /// 冗長な腕（それより上の腕で全部覆われている腕）の添字．
    pub redundant: Vec<usize>,
}

impl MatchCheck {
    pub fn is_exhaustive(&self) -> bool {
        self.missing.is_empty()
    }
}

/// 足りない値の例の数の上限．
pub const WITNESS_LIMIT: usize = 64;

/// 足りない値の例．表示は仕様のパターンの書き方に合わせる．
#[derive(Clone, Debug, PartialEq)]
pub enum Witness {
    Wild,
    Lit(Lit),
    Variant {
        name: SmolStr,
        args: Vec<Witness>,
    },
    Tuple(Vec<Witness>),
    /// `rest` なら `[_, ..]`（長さ `elems.len()` 以上）．
    List {
        elems: Vec<Witness>,
        rest: bool,
    },
}

impl fmt::Display for Witness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn comma_sep(f: &mut fmt::Formatter<'_>, ws: &[Witness]) -> fmt::Result {
            for (i, w) in ws.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{w}")?;
            }
            Ok(())
        }
        match self {
            Witness::Wild => f.write_str("_"),
            Witness::Lit(lit) => write!(f, "{lit}"),
            Witness::Variant { name, args } => {
                f.write_str(name)?;
                if !args.is_empty() {
                    f.write_str("(")?;
                    comma_sep(f, args)?;
                    f.write_str(")")?;
                }
                Ok(())
            }
            Witness::Tuple(ws) => {
                f.write_str("(")?;
                comma_sep(f, ws)?;
                f.write_str(")")
            }
            Witness::List { elems, rest } => {
                f.write_str("[")?;
                comma_sep(f, elems)?;
                if *rest {
                    if !elems.is_empty() {
                        f.write_str(", ")?;
                    }
                    f.write_str("..")?;
                }
                f.write_str("]")
            }
        }
    }
}

/// match を検査する．`ty` は照合される値の型．
pub fn check_match<C: CtorTable>(table: &C, ty: &C::Ty, arms: &[Arm]) -> MatchCheck {
    let cx = Cx { table };
    let tys = [ty.clone()];
    let mut rows: Vec<Row<'_>> = Vec::new();
    let mut redundant = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        if cx.useful(&rows, &[&arm.pat], &tys, 1).is_empty() {
            redundant.push(i);
        }
        // ガード付きの腕は，それより下の腕を覆わない．
        if !arm.guarded {
            rows.push(vec![&arm.pat]);
        }
    }
    let missing = cx
        .useful(&rows, &[&WILD], &tys, WITNESS_LIMIT)
        .into_iter()
        .map(|mut ws| ws.pop().expect("1列の witness"))
        .collect();
    MatchCheck { missing, redundant }
}

static WILD: Pat = Pat::Wild;

type Row<'p> = Vec<&'p Pat>;

/// 列を分けるときの構成子．リストは長さで分け，リテラルは現れた値と「それ以外」に分ける．
#[derive(Clone, Debug)]
enum Ctor<'p> {
    Variant(usize),
    Tuple,
    /// 長さちょうど n のリスト．
    ListFixed(usize),
    /// 長さ n 以上のリスト．n は列のどの固定長よりも長く，どの `[.., ..]` の前置部より短くない．
    ListAtLeast(usize),
    Lit(&'p Lit),
    /// 列に現れていないリテラル全部．ワイルドカードだけが覆う．
    Other,
}

struct Cx<'a, C> {
    table: &'a C,
}

impl<C: CtorTable> Cx<'_, C> {
    /// `rows` に対して `q` が有用なら，`q` に合い `rows` のどの行にも合わない値の例を返す．
    /// 有用でなければ空．例は `limit` 個までで打ち切る．
    fn useful<'p>(
        &self,
        rows: &[Row<'p>],
        q: &[&'p Pat],
        tys: &[C::Ty],
        limit: usize,
    ) -> Vec<Vec<Witness>> {
        let Some((&head, q_rest)) = q.split_first() else {
            return if rows.is_empty() {
                vec![vec![]]
            } else {
                vec![]
            };
        };
        let shape = self.table.shape(&tys[0]);
        let column_has_ctor = rows.iter().any(|r| !matches!(r[0], Pat::Wild));
        let ctors = split_ctors(&shape, rows.iter().map(|r| r[0]).chain([head]));

        let mut out = Vec::new();
        let mut missing = Vec::new();
        for ctor in ctors {
            if out.len() >= limit {
                break;
            }
            if !covers(head, &ctor) {
                continue;
            }
            // q の先頭が `_` で，列のどの構成子パターンもこの構成子を覆わないなら，
            // 特殊化した行列は既定の行列（先頭が `_` の行）と同じなので，まとめて調べる．
            // ワイルドカードの行で特殊化し続けると再帰的な型で止まらない．
            if matches!(head, Pat::Wild)
                && !rows
                    .iter()
                    .any(|r| !matches!(r[0], Pat::Wild) && covers(r[0], &ctor))
            {
                missing.push(ctor);
                continue;
            }
            let arity = arity(&shape, &ctor);
            let spec_rows: Vec<Row<'p>> = rows
                .iter()
                .filter(|r| covers(r[0], &ctor))
                .map(|r| specialize(r, &ctor, arity))
                .collect();
            let spec_q = specialize(q, &ctor, arity);
            let mut spec_tys = field_tys(&shape, &ctor);
            spec_tys.extend_from_slice(&tys[1..]);
            for ws in self.useful(&spec_rows, &spec_q, &spec_tys, limit - out.len()) {
                out.push(wrap(&shape, &ctor, ws));
            }
        }

        if !missing.is_empty() && out.len() < limit {
            let default_rows: Vec<Row<'p>> = rows
                .iter()
                .filter(|r| matches!(r[0], Pat::Wild))
                .map(|r| r[1..].to_vec())
                .collect();
            for ws in self.useful(&default_rows, q_rest, &tys[1..], limit - out.len()) {
                if !column_has_ctor {
                    // 列に構成子が1つもないなら，構成子を並べず `_` とだけ示す．
                    out.push(prepend(Witness::Wild, ws));
                } else {
                    for ctor in &missing {
                        out.push(prepend(wild_witness(&shape, ctor), ws.clone()));
                    }
                }
            }
        }
        out.truncate(limit);
        out
    }
}

/// 列の先頭のパターンから，その型の値を余さず分ける構成子の並びを作る．
fn split_ctors<'p, T>(shape: &TyShape<T>, heads: impl Iterator<Item = &'p Pat>) -> Vec<Ctor<'p>> {
    match shape {
        TyShape::Enum(variants) => (0..variants.len()).map(Ctor::Variant).collect(),
        TyShape::Tuple(_) => vec![Ctor::Tuple],
        TyShape::List(_) => {
            // 固定長の最大より長く，`[.., ..]` の前置部の最大以上の長さ n をとると，
            // n 以上の長さはどれも同じ行に合うので，0..n と「n 以上」で全部を分けられる．
            let mut n = 0;
            for p in heads {
                if let Pat::List { prefix, rest } = p {
                    n = n.max(if *rest {
                        prefix.len()
                    } else {
                        prefix.len() + 1
                    });
                }
            }
            (0..n)
                .map(Ctor::ListFixed)
                .chain([Ctor::ListAtLeast(n)])
                .collect()
        }
        TyShape::Opaque => {
            let mut lits: Vec<&'p Lit> = Vec::new();
            for p in heads {
                if let Pat::Lit(lit) = p
                    && !lits.contains(&lit)
                {
                    lits.push(lit);
                }
            }
            lits.into_iter()
                .map(Ctor::Lit)
                .chain([Ctor::Other])
                .collect()
        }
    }
}

/// パターン `p` が構成子 `ctor` の値を全部覆うか（`ctor` の中身は問わない）．
fn covers(p: &Pat, ctor: &Ctor<'_>) -> bool {
    match (p, ctor) {
        (Pat::Wild, _) => true,
        (Pat::Variant { variant, .. }, Ctor::Variant(v)) => variant == v,
        (Pat::Tuple(_), Ctor::Tuple) => true,
        (
            Pat::List {
                prefix,
                rest: false,
            },
            Ctor::ListFixed(n),
        ) => prefix.len() == *n,
        (Pat::List { rest: false, .. }, Ctor::ListAtLeast(_)) => false,
        (Pat::List { prefix, rest: true }, Ctor::ListFixed(n) | Ctor::ListAtLeast(n)) => {
            prefix.len() <= *n
        }
        (Pat::Lit(a), Ctor::Lit(b)) => a == *b,
        (Pat::Lit(_), Ctor::Other) => false,
        _ => unreachable!("型検査済みのパターンと型が食い違っている: {p:?} / {ctor:?}"),
    }
}

fn arity<T>(shape: &TyShape<T>, ctor: &Ctor<'_>) -> usize {
    match (shape, ctor) {
        (TyShape::Enum(variants), Ctor::Variant(v)) => variants[*v].fields.len(),
        (TyShape::Tuple(tys), Ctor::Tuple) => tys.len(),
        (_, Ctor::ListFixed(n) | Ctor::ListAtLeast(n)) => *n,
        _ => 0,
    }
}

fn field_tys<T: Clone>(shape: &TyShape<T>, ctor: &Ctor<'_>) -> Vec<T> {
    match (shape, ctor) {
        (TyShape::Enum(variants), Ctor::Variant(v)) => variants[*v].fields.clone(),
        (TyShape::Tuple(tys), Ctor::Tuple) => tys.clone(),
        (TyShape::List(elem), Ctor::ListFixed(n) | Ctor::ListAtLeast(n)) => vec![elem.clone(); *n],
        _ => vec![],
    }
}

/// 先頭の列を `ctor` で特殊化した行．先頭は `ctor` のフィールド `arity` 個に置き換わる．
/// 呼ぶ前に `covers(row[0], ctor)` を確かめておく．
fn specialize<'p>(row: &[&'p Pat], ctor: &Ctor<'_>, arity: usize) -> Row<'p> {
    let mut out: Row<'p> = vec![&WILD; arity];
    match row[0] {
        Pat::Wild | Pat::Lit(_) => {}
        Pat::Variant { fields, .. } => {
            for (i, p) in fields {
                out[*i] = p;
            }
        }
        Pat::Tuple(ps) => {
            for (slot, p) in out.iter_mut().zip(ps) {
                *slot = p;
            }
        }
        Pat::List { prefix, .. } => {
            // `[x, ..rest]` の残りはワイルドカード．
            for (slot, p) in out.iter_mut().zip(prefix) {
                *slot = p;
            }
        }
    }
    debug_assert!(covers(row[0], ctor));
    out.extend_from_slice(&row[1..]);
    out
}

/// 特殊化した列の例から，先頭の `arity` 個を `ctor` にまとめ直す．
fn wrap<T>(shape: &TyShape<T>, ctor: &Ctor<'_>, mut ws: Vec<Witness>) -> Vec<Witness> {
    let rest = ws.split_off(arity(shape, ctor));
    let head = match (shape, ctor) {
        (TyShape::Enum(variants), Ctor::Variant(v)) => Witness::Variant {
            name: variants[*v].name.clone(),
            args: ws,
        },
        (_, Ctor::Tuple) => Witness::Tuple(ws),
        (_, Ctor::ListFixed(_)) => Witness::List {
            elems: ws,
            rest: false,
        },
        (_, Ctor::ListAtLeast(_)) => Witness::List {
            elems: ws,
            rest: true,
        },
        (_, Ctor::Lit(lit)) => Witness::Lit((*lit).clone()),
        _ => Witness::Wild,
    };
    prepend(head, rest)
}

/// フィールドを全部 `_` にした `ctor` の例（`Rect(_, _)`，`[_, ..]`）．
fn wild_witness<T>(shape: &TyShape<T>, ctor: &Ctor<'_>) -> Witness {
    let ws = vec![Witness::Wild; arity(shape, ctor)];
    let mut out = wrap(shape, ctor, ws);
    out.pop().expect("構成子1つ分の witness")
}

fn prepend(head: Witness, mut rest: Vec<Witness>) -> Vec<Witness> {
    rest.insert(0, head);
    rest
}

#[cfg(test)]
mod tests;
