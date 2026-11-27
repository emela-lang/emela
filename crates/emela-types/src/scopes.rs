//! Where type and set variables are made, for telling when a capability leaves what made it
//! (`[effect.escape]`).
//!
//! A capability lives inside the `with` that made it (or the closure that received a quantified
//! set, or the function a provider by name passes it to). Each of these opens a scope; scopes
//! nest as the source does. Every type variable and set variable records the scope it was made
//! in. A variable made outside a scope stands for a type fixed outside it (the type argument of
//! an outer `State`, the parameter types of an operation of an outer handler), so it cannot
//! contain a capability of that scope: when it does, the capability leaves (E4405).
//!
//! - Binding a type variable to a type lowers the scope of the variables in that type to the
//!   nearest scope enclosing both: they are now reachable from the outer variable.
//! - A capability written in a type bound to a variable made outside its scope leaves at once.
//! - A set variable whose solution has a capability of a scope that does not enclose the
//!   variable's scope leaves; this is checked once the sets are solved.

use emela_resolve::hir::{ExprId, Span};
use rustc_hash::FxHashMap;

use crate::ty::{CapElem, CapId, RowVar, TyVar};

/// The index of a scope. `0` is the body of the definition, outside every `with`.
pub(crate) type ScopeId = u32;

pub(crate) struct Scopes {
    /// The parent of each scope; the root has none.
    parents: Vec<Option<ScopeId>>,
    /// The expression that opens each scope: a `with`, a closure, or the body of a local fn.
    by_owner: FxHashMap<ExprId, ScopeId>,
    current: ScopeId,
    ty_vars: Vec<ScopeId>,
    row_vars: Vec<ScopeId>,
    /// The span of the expression being inferred when each set variable was made, used when no
    /// better place is known.
    row_spans: Vec<Option<Span>>,
    /// Capabilities that a binding put into a variable made outside their scope, with where.
    pub(crate) escapes: Vec<(CapElem, Option<Span>)>,
    /// The scope each skolem ([`CapId::Skolem`]) was made in.
    skolems: Vec<ScopeId>,
}

impl Default for Scopes {
    fn default() -> Self {
        Scopes {
            parents: vec![None],
            by_owner: FxHashMap::default(),
            current: 0,
            ty_vars: Vec::new(),
            row_vars: Vec::new(),
            row_spans: Vec::new(),
            escapes: Vec::new(),
            skolems: Vec::new(),
        }
    }
}

impl Scopes {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Enters the scope that `owner` opens, inside the current one. Entering the same owner
    /// again (the effect sets are computed after the types) reuses its scope.
    pub(crate) fn enter(&mut self, owner: ExprId) {
        let id = match self.by_owner.get(&owner) {
            Some(&id) => id,
            None => {
                let id = self.parents.len() as ScopeId;
                self.parents.push(Some(self.current));
                self.by_owner.insert(owner, id);
                id
            }
        };
        self.current = id;
    }

    /// Leaves the current scope.
    pub(crate) fn exit(&mut self) {
        self.current = self.parents[self.current as usize].unwrap_or(0);
    }

    /// Back to the body of a definition, for the next one.
    pub(crate) fn reset(&mut self) {
        self.current = 0;
    }

    /// A new skolem in the current scope.
    pub(crate) fn new_skolem(&mut self) -> u32 {
        self.skolems.push(self.current);
        (self.skolems.len() - 1) as u32
    }

    pub(crate) fn new_ty_var(&mut self, v: TyVar) {
        let i = v.index() as usize;
        if self.ty_vars.len() <= i {
            self.ty_vars.resize(i + 1, 0);
        }
        self.ty_vars[i] = self.current;
    }

    pub(crate) fn new_row_var(&mut self, v: RowVar, span: Option<Span>) {
        let i = v.index() as usize;
        if self.row_vars.len() <= i {
            self.row_vars.resize(i + 1, 0);
            self.row_spans.resize(i + 1, None);
        }
        self.row_vars[i] = self.current;
        self.row_spans[i] = span;
    }

    /// Where a capability that leaves through the set variable `v` is reported, when nothing
    /// better is known.
    pub(crate) fn set_row_span(&mut self, v: RowVar, span: Span) {
        if let Some(s) = self.row_spans.get_mut(v.index() as usize) {
            *s = Some(span);
        }
    }

    pub(crate) fn ty_var(&self, v: TyVar) -> ScopeId {
        self.ty_vars.get(v.index() as usize).copied().unwrap_or(0)
    }

    pub(crate) fn row_var(&self, v: RowVar) -> ScopeId {
        self.row_vars.get(v.index() as usize).copied().unwrap_or(0)
    }

    pub(crate) fn row_span(&self, v: RowVar) -> Option<Span> {
        self.row_spans.get(v.index() as usize).copied().flatten()
    }

    /// Moves the type variable `v` out to `scope` if that is further out.
    pub(crate) fn lower_ty_var(&mut self, v: TyVar, scope: ScopeId) {
        let lca = self.common(self.ty_var(v), scope);
        if let Some(s) = self.ty_vars.get_mut(v.index() as usize) {
            *s = lca;
        }
    }

    /// Moves the set variable `v` out to `scope` if that is further out.
    pub(crate) fn lower_row_var(&mut self, v: RowVar, scope: ScopeId) {
        let lca = self.common(self.row_var(v), scope);
        if let Some(s) = self.row_vars.get_mut(v.index() as usize) {
            *s = lca;
        }
    }

    /// The nearest scope that encloses both.
    pub(crate) fn common(&self, a: ScopeId, b: ScopeId) -> ScopeId {
        let mut x = a;
        loop {
            if self.encloses(x, b) {
                return x;
            }
            match self.parents[x as usize] {
                Some(p) => x = p,
                None => return 0,
            }
        }
    }

    /// Whether `outer` is `inner` or encloses it.
    pub(crate) fn encloses(&self, outer: ScopeId, inner: ScopeId) -> bool {
        let mut x = Some(inner);
        while let Some(s) = x {
            if s == outer {
                return true;
            }
            x = self.parents[s as usize];
        }
        false
    }

    /// The scope a capability lives in, if it has one in this group of definitions.
    pub(crate) fn home(&self, cap: &CapElem) -> Option<ScopeId> {
        let owner = match cap.id {
            CapId::Item { with, .. } => with,
            CapId::Named { provider, .. } => provider,
            CapId::Rigid { binder, .. } => binder,
            CapId::Skolem(i) => return self.skolems.get(i as usize).copied(),
            CapId::SelfOf(_) | CapId::Quant(_) => return None,
        };
        self.by_owner.get(&owner).copied()
    }

    /// Whether `cap` can be in something made in `scope`.
    pub(crate) fn fits(&self, cap: &CapElem, scope: ScopeId) -> bool {
        self.home(cap).is_none_or(|home| self.encloses(home, scope))
    }
}
