//! Match exhaustiveness and redundant-arm checks.
//!
//! Based on Maranget's usefulness ("Warnings for pattern matching", 2007): whether a vector q is
//! useful with respect to a pattern matrix P, that is, whether some value matches q but no row of
//! P, along with an example of such a value (a witness).
//!
//! - Exhaustiveness: values are missing if `_` is useful against the matrix of unguarded arms.
//! - Redundancy: arm i is redundant if its pattern is not useful against the matrix of the
//!   unguarded arms above it.
//!
//! Independent of the syntax tree and of type inference. The input is this module's own [`Pat`]
//! and a [`CtorTable`] that looks up constructor shapes from types; type inference converts to
//! them. Field types are looked up one level at a time on each specialization, so recursive types
//! terminate.

use std::fmt;

use smol_str::SmolStr;

/// Assumes name resolution and type checking are done.
#[derive(Clone, Debug, PartialEq)]
pub enum Pat {
    /// `_` and a variable `x`.
    Wild,
    /// Literals `0` `1.5` `"a"` and constants `MAX_SIZE` (constants are passed as their values).
    Lit(Lit),
    /// Enum constructors and `type`. `variant` is the constructor's index in [`TyShape::Enum`].
    /// `fields` pairs a field's position in declaration order with a pattern; fields not written
    /// are wildcards (`User(name:, ..)`). Omitting a named field without `..` is an error
    /// (`[fn.match]`), but the type checker reports it; here it is only a wildcard.
    Variant {
        variant: usize,
        fields: Vec<(usize, Pat)>,
    },
    /// Tuples `(a, b)` and `()`.
    Tuple(Vec<Pat>),
    /// `[x, y]` has `rest: false`; `[x, ..rest]` has `rest: true`.
    List { prefix: Vec<Pat>, rest: bool },
}

impl Pat {
    /// A constructor pattern with all fields by position, `Rect(w, h)`.
    pub fn variant(variant: usize, args: Vec<Pat>) -> Pat {
        Pat::Variant {
            variant,
            fields: args.into_iter().enumerate().collect(),
        }
    }

    /// A fixed-length list `[x, y]`.
    pub fn list(elems: Vec<Pat>) -> Pat {
        Pat::List {
            prefix: elems,
            rest: false,
        }
    }

    /// A list of length n or more, `[x, ..rest]`.
    pub fn list_rest(prefix: Vec<Pat>) -> Pat {
        Pat::List { prefix, rest: true }
    }
}

/// Literal values are infinitely many, so literals alone can never be exhaustive.
#[derive(Clone, Debug)]
pub enum Lit {
    Int(i64),
    /// Compared by value (`0.0` and `-0.0` are equal). NaNs are equal to each other.
    Float(f64),
    Str(SmolStr),
}

impl PartialEq for Lit {
    fn eq(&self, other: &Lit) -> bool {
        match (self, other) {
            (Lit::Int(a), Lit::Int(b)) => a == b,
            // Otherwise an arm with a NaN constant would never match itself and always be
            // redundant.
            (Lit::Float(a), Lit::Float(b)) => a == b || (a.is_nan() && b.is_nan()),
            (Lit::Str(a), Lit::Str(b)) => a == b,
            _ => false,
        }
    }
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

/// Returned by [`CtorTable::shape`].
#[derive(Clone, Debug)]
pub enum TyShape<T> {
    /// A type whose constructors cannot be enumerated (`Int`, `Float`, `String`, functions,
    /// ...). Literals can match it, but only a wildcard makes it exhaustive.
    Opaque,
    /// Also represents `Bool` (`True` / `False`), `Option`, and a single-constructor `type`.
    Enum(Vec<VariantShape<T>>),
    /// `()` has no elements.
    Tuple(Vec<T>),
    List(T),
}

/// `fields` holds the field types in declaration order, with type arguments instantiated.
#[derive(Clone, Debug)]
pub struct VariantShape<T> {
    pub name: SmolStr,
    pub fields: Vec<T>,
}

/// Implemented by type inference.
pub trait CtorTable {
    type Ty: Clone;

    fn shape(&self, ty: &Self::Ty) -> TyShape<Self::Ty>;
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pat,
    /// Whether it has an `if` guard.
    pub guarded: bool,
}

/// Not a [`crate::TypeError`]: the checker knows no spans, so type inference turns this into
/// diagnostics where it connects the check (`coverage.rs`).
#[derive(Clone, Debug, PartialEq)]
pub struct MatchCheck {
    /// Examples of missing values; empty if exhaustive. Truncated at [`WITNESS_LIMIT`].
    pub missing: Vec<Witness>,
    /// Indices of redundant arms (arms fully covered by the arms above them).
    pub redundant: Vec<usize>,
}

impl MatchCheck {
    pub fn is_exhaustive(&self) -> bool {
        self.missing.is_empty()
    }
}

pub const WITNESS_LIMIT: usize = 64;

/// Displayed in the spec's pattern syntax.
#[derive(Clone, Debug, PartialEq)]
pub enum Witness {
    Wild,
    Lit(Lit),
    Variant {
        name: SmolStr,
        args: Vec<Witness>,
    },
    Tuple(Vec<Witness>),
    /// With `rest`, `[_, ..]` (length `elems.len()` or more).
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

/// `ty` is the type of the matched value.
pub fn check_match<C: CtorTable>(table: &C, ty: &C::Ty, arms: &[Arm]) -> MatchCheck {
    let cx = Cx { table };
    let tys = [ty.clone()];
    let mut rows: Vec<Row<'_>> = Vec::new();
    let mut redundant = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        if cx.useful(&rows, &[&arm.pat], &tys, 1).is_empty() {
            redundant.push(i);
        }
        // A guarded arm does not cover the arms below it.
        if !arm.guarded {
            rows.push(vec![&arm.pat]);
        }
    }
    let missing = cx
        .useful(&rows, &[&WILD], &tys, WITNESS_LIMIT)
        .into_iter()
        .map(|mut ws| ws.pop().expect("a one-column witness"))
        .collect();
    MatchCheck { missing, redundant }
}

static WILD: Pat = Pat::Wild;

type Row<'p> = Vec<&'p Pat>;

/// Constructors for splitting a column. Lists split by length; literals split into the values
/// that appear and "everything else".
#[derive(Clone, Debug)]
enum Ctor<'p> {
    Variant(usize),
    Tuple,
    /// A list of length exactly n.
    ListFixed(usize),
    /// A list of length n or more. n is longer than every fixed length in the column and no
    /// shorter than any `[.., ..]` prefix.
    ListAtLeast(usize),
    Lit(&'p Lit),
    /// All literals not in the column. Only a wildcard covers it.
    Other,
}

struct Cx<'a, C> {
    table: &'a C,
}

impl<C: CtorTable> Cx<'_, C> {
    /// If `q` is useful against `rows`, returns examples of values that match `q` but no row of
    /// `rows`; otherwise empty. At most `limit` examples.
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
            // If q starts with `_` and no constructor pattern in the column covers this
            // constructor, the specialized matrix equals the default matrix (rows starting with
            // `_`), so check them together. Specializing on wildcard rows forever would not
            // terminate on recursive types.
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
                    // No constructor in the column: show just `_` rather than listing them.
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

/// Builds constructors that partition all values of the type, from the column's head patterns.
fn split_ctors<'p, T>(shape: &TyShape<T>, heads: impl Iterator<Item = &'p Pat>) -> Vec<Ctor<'p>> {
    match shape {
        TyShape::Enum(variants) => (0..variants.len()).map(Ctor::Variant).collect(),
        TyShape::Tuple(_) => vec![Ctor::Tuple],
        TyShape::List(_) => {
            // Take n longer than the longest fixed length and at least the longest `[.., ..]`
            // prefix. Every length of n or more matches the same rows, so 0..n plus "n or more"
            // covers everything.
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

/// Whether `p` covers every value of `ctor`, regardless of its contents.
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
        _ => unreachable!("type-checked pattern does not match its type: {p:?} / {ctor:?}"),
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

/// The row with its first column specialized by `ctor`: the head is replaced by `arity` fields
/// of `ctor`. Check `covers(row[0], ctor)` before calling.
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
            // The remainder of `[x, ..rest]` is wildcards.
            for (slot, p) in out.iter_mut().zip(prefix) {
                *slot = p;
            }
        }
    }
    debug_assert!(covers(row[0], ctor));
    out.extend_from_slice(&row[1..]);
    out
}

/// Regroups the first `arity` items of a specialized column's witness into `ctor`.
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

/// A witness of `ctor` with all fields `_` (`Rect(_, _)`, `[_, ..]`).
fn wild_witness<T>(shape: &TyShape<T>, ctor: &Ctor<'_>) -> Witness {
    let ws = vec![Witness::Wild; arity(shape, ctor)];
    let mut out = wrap(shape, ctor, ws);
    out.pop().expect("one constructor's witness")
}

fn prepend(head: Witness, mut rest: Vec<Witness>) -> Vec<Witness> {
    rest.insert(0, head);
    rest
}

#[cfg(test)]
mod tests;
