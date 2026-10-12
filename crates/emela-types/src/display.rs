//! 型の表示．仕様の書き方（`List[Int]`，`fn(Int) -> Bool fails NotFound`，`(Int, String)`）に合わせる．
//!
//! 名前付き型とエラーのタグの名前は `TyCons` にあるので，表示には表を渡す．
//! 型変数は束縛を辿らずにそのまま `?0` と出す．辿った形を出すときは
//! `InferCtx::resolve` を通してから表示する．
//!
//! 関数型のエラーの集合と効果の集合は，空なら `fails` 節と `use` 節ごと省く．ただし引数の
//! 型の中では，省くと「何でもよい」と読めるので `fails {}` と `use {}` と出す（9.2）．
//! 戻り値が関数型で外側の関数型に `fails` 節か `use` 節があるときは，付き先が分かるように
//! 戻り値を括弧で囲む（5.5）．
//!
//! 能力の値の型（`Ty::Cap`）はソースに書けないので，それを作る式の形で `use Logger` と出す．

use std::fmt;

use crate::row::{Row, RowElem, SetKind};
use crate::scheme::Scheme;
use crate::ty::{Ty, TyCons, TyParam};

impl Row {
    /// `NotFound | E` の形．空なら `{}`．
    pub fn display<'a>(&'a self, cons: &'a TyCons) -> RowDisplay<'a> {
        RowDisplay {
            row: self,
            effects: false,
            ty: TyDisplay {
                ty: &Ty::Never,
                cons,
                bound: &[],
                in_param: false,
            },
        }
    }

    /// [`Self::display`] と同じ．`Bound(i)` の名前を `bound` から引く．
    pub fn display_with<'a>(&'a self, cons: &'a TyCons, bound: &'a [TyParam]) -> RowDisplay<'a> {
        RowDisplay {
            row: self,
            effects: false,
            ty: TyDisplay {
                ty: &Ty::Never,
                cons,
                bound,
                in_param: false,
            },
        }
    }
}

impl Row {
    /// 効果の集合の形（17.4）．要素が1つなら `Logger`，2つ以上なら `{ Logger, Clock }`，
    /// 空なら `{}`．`Bound(i)` の名前を `bound` から引く．
    pub fn display_effects<'a>(&'a self, cons: &'a TyCons, bound: &'a [TyParam]) -> RowDisplay<'a> {
        RowDisplay {
            effects: true,
            ..self.display_with(cons, bound)
        }
    }
}

impl Ty {
    /// [`Self::display`] と同じ．`Bound(i)` の名前を `bound` から引く．
    pub fn display_with<'a>(&'a self, cons: &'a TyCons, bound: &'a [TyParam]) -> TyDisplay<'a> {
        TyDisplay {
            ty: self,
            cons,
            bound,
            in_param: false,
        }
    }
}

/// 集合の表示．エラーの集合は要素を ` | ` で区切る．効果の集合は2つ以上なら
/// `{ A, B }` と波括弧で囲み，`, ` で区切る．
pub struct RowDisplay<'a> {
    row: &'a Row,
    /// 効果の集合の形で出すか．
    effects: bool,
    /// 要素の中の型を表示するための文脈（`ty` は使わない）．
    ty: TyDisplay<'a>,
}

impl fmt::Display for RowDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.row.is_empty() {
            return f.write_str("{}");
        }
        let braces = self.effects && self.row.elems().len() > 1;
        if braces {
            f.write_str("{ ")?;
        }
        for (i, e) in self.row.elems().iter().enumerate() {
            if i > 0 {
                f.write_str(if self.effects { ", " } else { " | " })?;
            }
            match e {
                RowElem::Tag(def) => match self.ty.cons.tag_name(*def) {
                    Some(name) => f.write_str(name)?,
                    None => write!(f, "#{}", def.into_raw())?,
                },
                RowElem::Bound(i) => match self.ty.bound.get(*i as usize) {
                    Some(p) => f.write_str(&p.name)?,
                    None => write!(f, "'{i}")?,
                },
                RowElem::Of(ty) => write!(f, "{}", self.ty.child(ty))?,
                RowElem::Var(v) => write!(f, "?e{}", v.index())?,
            }
        }
        if braces {
            f.write_str(" }")?;
        }
        Ok(())
    }
}

impl Ty {
    pub fn display<'a>(&'a self, cons: &'a TyCons) -> TyDisplay<'a> {
        TyDisplay {
            ty: self,
            cons,
            bound: &[],
            in_param: false,
        }
    }
}

impl Scheme {
    pub fn display<'a>(&'a self, cons: &'a TyCons) -> SchemeDisplay<'a> {
        SchemeDisplay { scheme: self, cons }
    }
}

pub struct TyDisplay<'a> {
    ty: &'a Ty,
    cons: &'a TyCons,
    /// `Ty::Bound(i)` の名前．範囲外なら `'i` と出す．
    bound: &'a [TyParam],
    /// 関数型の引数の型の中か．空のエラーの集合を `fails {}` と出す．
    in_param: bool,
}

impl TyDisplay<'_> {
    fn child<'b>(&'b self, ty: &'b Ty) -> TyDisplay<'b> {
        TyDisplay {
            ty,
            cons: self.cons,
            bound: self.bound,
            in_param: self.in_param,
        }
    }

    fn list(&self, f: &mut fmt::Formatter<'_>, tys: &[Ty]) -> fmt::Result {
        for (i, ty) in tys.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", self.child(ty))?;
        }
        Ok(())
    }

    fn params(&self, f: &mut fmt::Formatter<'_>, tys: &[Ty]) -> fmt::Result {
        for (i, ty) in tys.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            let child = TyDisplay {
                in_param: true,
                ..self.child(ty)
            };
            write!(f, "{child}")?;
        }
        Ok(())
    }
}

impl fmt::Display for TyDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.ty {
            Ty::Prim(p) => f.write_str(p.name()),
            Ty::Never => f.write_str("Never"),
            Ty::Error => f.write_str("{unknown}"),
            Ty::Var(v) => write!(f, "?{}", v.index()),
            Ty::Bound(i) => match self.bound.get(*i as usize) {
                Some(p) => f.write_str(&p.name),
                None => write!(f, "'{i}"),
            },
            Ty::Cap(effect) => match self.cons.tag_name(*effect) {
                Some(name) => write!(f, "use {name}"),
                None => write!(f, "use #{}", effect.into_raw()),
            },
            Ty::Fn(fun) => {
                f.write_str("fn(")?;
                self.params(f, &fun.params)?;
                let show_fails = !fun.fails.is_empty() || self.in_param;
                let show_uses = !fun.uses.is_empty() || self.in_param;
                match &fun.ret {
                    Ty::Fn(_) if show_fails || show_uses => {
                        write!(f, ") -> ({})", self.child(&fun.ret))?
                    }
                    ret => write!(f, ") -> {}", self.child(ret))?,
                }
                if show_fails {
                    let row = RowDisplay {
                        row: &fun.fails,
                        effects: false,
                        ty: self.child(&fun.ret),
                    };
                    write!(f, " fails {row}")?;
                }
                if show_uses {
                    let row = RowDisplay {
                        row: &fun.uses,
                        effects: true,
                        ty: self.child(&fun.ret),
                    };
                    write!(f, " use {row}")?;
                }
                Ok(())
            }
            // 名前付き型の集合の型引数（`Handler[NotFound | Timeout, { Db, Logger }]`）．
            Ty::Set(kind, row) => {
                let row = RowDisplay {
                    row,
                    effects: *kind == SetKind::Effects,
                    ty: self.child(self.ty),
                };
                write!(f, "{row}")
            }
            Ty::Tuple(elems) => {
                f.write_str("(")?;
                self.list(f, elems)?;
                f.write_str(")")
            }
            Ty::List(elem) => write!(f, "List[{}]", self.child(elem)),
            Ty::Named(id, args) => {
                f.write_str(&self.cons[*id].name)?;
                if !args.is_empty() {
                    f.write_str("[")?;
                    self.list(f, args)?;
                    f.write_str("]")?;
                }
                Ok(())
            }
        }
    }
}

/// `∀ A: Ord + Show, B. fn(A) -> B`．量化しないスキームは型だけを出す．
pub struct SchemeDisplay<'a> {
    scheme: &'a Scheme,
    cons: &'a TyCons,
}

impl fmt::Display for SchemeDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let params = &self.scheme.params;
        if !params.is_empty() {
            f.write_str("∀ ")?;
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                f.write_str(&p.name)?;
                for (j, b) in p.bounds.iter().enumerate() {
                    f.write_str(if j == 0 { ": " } else { " + " })?;
                    f.write_str(b)?;
                }
            }
            f.write_str(". ")?;
        }
        let ty = TyDisplay {
            ty: &self.scheme.ty,
            cons: self.cons,
            bound: params,
            in_param: false,
        };
        write!(f, "{ty}")
    }
}
