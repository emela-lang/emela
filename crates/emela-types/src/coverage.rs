//! 網羅性の検査（[`crate::exhaustiveness`]）を型推論の結果につなぐ．
//!
//! HIR のパターンを検査用のパターンに変換し，型から構成子の形を引く表を実装する．
//! 検査は，定義のまとまりの推論が終わって型が決まった後に行う（[`crate::body`]）．

use emela_resolve::hir::{DefId, ExprKind, Item, Literal, PatId, PatKind, Program, Res, UnaryOp};
use smol_str::SmolStr;

use crate::exhaustiveness::{CtorTable, Lit, Pat, TyShape, VariantShape};
use crate::infer::substitute_bound;
use crate::lower::TyTable;
use crate::signature::Signatures;
use crate::ty::{Prim, Ty};

/// 型から構成子の形を引く表．型は単一化の束縛を辿った形で渡す．
pub(crate) struct Shapes<'a> {
    pub(crate) program: &'a Program,
    pub(crate) ty_table: &'a TyTable,
    pub(crate) sigs: &'a Signatures,
}

impl CtorTable for Shapes<'_> {
    type Ty = Ty;

    fn shape(&self, ty: &Ty) -> TyShape<Ty> {
        match ty {
            Ty::Prim(Prim::Bool) => match self.program.prelude_def("Bool") {
                Some(bool_def) => TyShape::Enum(self.variants(bool_def, &[])),
                None => TyShape::Opaque,
            },
            Ty::Prim(Prim::Unit) => TyShape::Tuple(Vec::new()),
            Ty::Tuple(elems) => TyShape::Tuple(elems.clone()),
            Ty::List(elem) => TyShape::List((**elem).clone()),
            Ty::Named(id, args) => {
                let def = self.ty_table.def(*id);
                match self.program.item(def) {
                    Some(Item::Enum(_)) => TyShape::Enum(self.variants(def, args)),
                    Some(Item::Type(item)) => match item.ctor {
                        Some(ctor) => TyShape::Enum(vec![self.variant(ctor, args)]),
                        None => TyShape::Opaque,
                    },
                    Some(Item::Error(_)) => TyShape::Enum(vec![self.variant(def, args)]),
                    _ => TyShape::Opaque,
                }
            }
            _ => TyShape::Opaque,
        }
    }
}

impl Shapes<'_> {
    /// enum のバリアントを宣言の順に並べる．
    fn variants(&self, enum_def: DefId, args: &[Ty]) -> Vec<VariantShape<Ty>> {
        match self.program.item(enum_def) {
            Some(Item::Enum(item)) => item
                .variants
                .iter()
                .map(|&v| self.variant(v, args))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// 構成子1つの形．フィールドの型は構成子の宣言の型から引き，型引数を代入する．
    fn variant(&self, ctor: DefId, args: &[Ty]) -> VariantShape<Ty> {
        let fields = match self.sigs.get(ctor).map(|sig| &sig.scheme.ty) {
            Some(Ty::Fn(f)) => f.params.iter().map(|p| substitute_bound(p, args)).collect(),
            _ => Vec::new(),
        };
        VariantShape {
            name: self.program.defs[ctor].name.clone(),
            fields,
        }
    }

    /// HIR のパターンを，型 `ty` の値に当てる検査用のパターンにする．
    ///
    /// 型に合わない部分（型検査が誤りを出している）は，検査が崩れないようワイルドカードにする．
    pub(crate) fn pat(&self, pat: PatId, ty: &Ty) -> Pat {
        let program = self.program;
        let shape = self.shape(ty);
        match (&program.pats[pat].kind, shape) {
            (PatKind::Wild | PatKind::Bind(_) | PatKind::Missing, _) => Pat::Wild,
            (PatKind::Lit(lit), TyShape::Opaque) => Pat::Lit(lit_value(lit, false)),
            (PatKind::Const(Res::Def(def)), TyShape::Opaque) => Pat::Lit(self.const_value(*def)),
            (PatKind::Unit, TyShape::Tuple(tys)) if tys.is_empty() => Pat::Tuple(Vec::new()),
            (PatKind::Tuple(elems), TyShape::Tuple(tys)) if elems.len() == tys.len() => Pat::Tuple(
                elems
                    .iter()
                    .zip(&tys)
                    .map(|(&p, ty)| self.pat(p, ty))
                    .collect(),
            ),
            (PatKind::List { elems, rest }, TyShape::List(elem_ty)) => Pat::List {
                prefix: elems.iter().map(|&p| self.pat(p, &elem_ty)).collect(),
                rest: rest.is_some(),
            },
            (
                PatKind::Ctor {
                    res: Res::Def(ctor),
                    args,
                    ..
                },
                TyShape::Enum(variants),
            ) => {
                let Some(variant) = self.variant_index(*ctor, ty) else {
                    return Pat::Wild;
                };
                let field_tys = &variants[variant].fields;
                let labels = self.sigs.get(*ctor).map(|sig| sig.labels.as_slice());
                let mut fields = Vec::new();
                let mut next_positional = 0;
                for arg in args.iter().flatten() {
                    let position = match &arg.label {
                        None => {
                            let i = next_positional;
                            next_positional += 1;
                            Some(i)
                        }
                        Some(label) => labels.and_then(|labels| {
                            labels.iter().position(|l| l.as_ref() == Some(&label.name))
                        }),
                    };
                    // 置けなかったフィールドは型検査が誤りを出している．
                    if let Some(i) = position
                        && let Some(field_ty) = field_tys.get(i)
                    {
                        fields.push((i, self.pat(arg.pat, field_ty)));
                    }
                }
                Pat::Variant { variant, fields }
            }
            _ => Pat::Wild,
        }
    }

    /// 型の定義（type，enum，error，Bool の enum）．
    fn ty_def(&self, ty: &Ty) -> Option<DefId> {
        match ty {
            Ty::Named(id, _) => Some(self.ty_table.def(*id)),
            Ty::Prim(Prim::Bool) => self.program.prelude_def("Bool"),
            _ => None,
        }
    }

    /// 構成子が，型 `ty` の構成子の並びの何番目か．構成子が `ty` を作るものでなければ `None`．
    fn variant_index(&self, ctor: DefId, ty: &Ty) -> Option<usize> {
        let ty_def = self.ty_def(ty)?;
        match self.program.item(ctor) {
            // error は自分自身が構成子．
            Some(Item::Error(_)) => (ctor == ty_def).then_some(0),
            Some(Item::Ctor(item)) if item.owner == ty_def => match self.program.item(ty_def) {
                Some(Item::Enum(e)) => e.variants.iter().position(|&v| v == ctor),
                _ => Some(0),
            },
            _ => None,
        }
    }

    /// const のパターンの値．値がリテラル（か負の数値リテラル）ならその値．
    /// それ以外の式はコンパイル時の評価がまだないので，その const だけに合う値として扱う．
    fn const_value(&self, def: DefId) -> Lit {
        let program = self.program;
        if let Some(Item::Const(item)) = program.item(def) {
            match &program.exprs[item.value].kind {
                ExprKind::Lit(lit) => return lit_value(lit, false),
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand,
                } => {
                    if let ExprKind::Lit(lit @ (Literal::Int(_) | Literal::Float(_))) =
                        &program.exprs[*operand].kind
                    {
                        return lit_value(lit, true);
                    }
                }
                _ => {}
            }
        }
        // 他のどのリテラルとも違う値．文字列の値には現れない文字で始める．
        Lit::Str(SmolStr::new(format!("\u{0}const {}", def.into_raw())))
    }
}

/// リテラルの値．範囲外の整数（型検査が誤りを出している）は，他と重ならない値にする．
fn lit_value(lit: &Literal, negative: bool) -> Lit {
    match lit {
        Literal::Int(text) => {
            let (digits, radix) = if let Some(hex) = text.strip_prefix("0x") {
                (hex, 16)
            } else if let Some(bin) = text.strip_prefix("0b") {
                (bin, 2)
            } else {
                (text.as_str(), 10)
            };
            match i64::from_str_radix(digits, radix) {
                Ok(n) => Lit::Int(if negative { -n } else { n }),
                Err(_) => Lit::Str(SmolStr::new(format!("\u{0}int {text}"))),
            }
        }
        Literal::Float(text) => match text.parse::<f64>() {
            Ok(x) => Lit::Float(if negative { -x } else { x }),
            Err(_) => Lit::Str(SmolStr::new(format!("\u{0}float {text}"))),
        },
        Literal::String(s) => Lit::Str(SmolStr::new(s)),
    }
}
