//! 型の表示．仕様の書き方（`List[Int]`，`fn(Int) -> Bool`，`(Int, String)`）に合わせる．
//!
//! 名前付き型の名前は `TyCons` にあるので，表示には表を渡す．
//! 型変数は束縛を辿らずにそのまま `?0` と出す．辿った形を出すときは
//! `InferCtx::resolve` を通してから表示する．

use std::fmt;

use crate::scheme::Scheme;
use crate::ty::{Ty, TyCons, TyParam};

impl Ty {
    pub fn display<'a>(&'a self, cons: &'a TyCons) -> TyDisplay<'a> {
        TyDisplay {
            ty: self,
            cons,
            bound: &[],
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
}

impl TyDisplay<'_> {
    fn child<'b>(&'b self, ty: &'b Ty) -> TyDisplay<'b> {
        TyDisplay {
            ty,
            cons: self.cons,
            bound: self.bound,
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
}

impl fmt::Display for TyDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.ty {
            Ty::Prim(p) => f.write_str(p.name()),
            Ty::Never => f.write_str("Never"),
            Ty::Var(v) => write!(f, "?{}", v.index()),
            Ty::Bound(i) => match self.bound.get(*i as usize) {
                Some(p) => f.write_str(&p.name),
                None => write!(f, "'{i}"),
            },
            Ty::Fn(fun) => {
                f.write_str("fn(")?;
                self.list(f, &fun.params)?;
                write!(f, ") -> {}", self.child(&fun.ret))
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
        };
        write!(f, "{ty}")
    }
}
