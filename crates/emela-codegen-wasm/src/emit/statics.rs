//! Static data (descriptors, string literals, nullary constructors, closures without captures)
//! and the value types of calls.

use super::*;

impl Builder<'_> {
    // ---- statics ----
    //
    // Descriptors are created on demand by shape (`DescKey`). A new lowering builds the
    // descriptor it needs with `desc` (and a static value with `nullary` or `string_literal`);
    // there is no central table to extend.

    pub(super) fn alloc_static(&mut self, size: u32) -> u32 {
        let align = (8 - (self.data.len() as u32 % 8)) % 8;
        self.data
            .extend(std::iter::repeat_n(0u8, (align + size) as usize));
        let off = self.data.len() as u32 - size;
        debug_assert!(
            off >= layout::STATIC_START,
            "static data below the scratch area"
        );
        off
    }

    pub(super) fn write_u32(&mut self, off: u32, v: u32) {
        self.data[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// The offset of a static string object: rc == 0, no slots, the length in the first word and
    /// the utf-8 bytes after it.
    pub(crate) fn string_literal(&mut self, bytes: &[u8]) -> u32 {
        self.string_object(bytes, true)
    }

    /// A string literal that no other use shares, so that the runtime can mark its `aux` word
    /// (the self-reference message of a const, spec 5.6).
    pub(super) fn unique_string_literal(&mut self, bytes: &[u8]) -> u32 {
        self.string_object(bytes, false)
    }

    pub(super) fn string_object(&mut self, bytes: &[u8], shared: bool) -> u32 {
        if shared && let Some(&o) = self.strs.get(bytes) {
            return o;
        }
        let o = self.alloc_static(HDR + 4 + bytes.len() as u32 + 1);
        let desc = self.string_desc();
        self.write_u32(o + 8, desc);
        self.write_u32(o + HDR, bytes.len() as u32);
        self.data[(o + HDR + 4) as usize..(o + HDR + 4) as usize + bytes.len()]
            .copy_from_slice(bytes);
        if shared {
            self.strs.insert(bytes.to_vec(), o);
        }
        o
    }

    /// The static cell of a const: its state, its value and defect, and the two messages the
    /// runtime wraps with (spec 5.6).
    pub(super) fn const_cell(&mut self, id: FnId, f: &Function) {
        let name = check::display_name(&f.name);
        let prefix = self.string_literal(format!("while evaluating const {name}: ").as_bytes());
        // The marker in `aux` travels with the defect (through `defect_take`, notes and
        // `defect_resume`). The string is unique to this const, so marking it cannot affect a
        // user string with the same bytes.
        let self_ref = self.unique_string_literal(
            format!("const {name} refers to itself while being evaluated").as_bytes(),
        );
        self.write_u32(self_ref + 4, 1);
        let cell = self.alloc_static(CONST_CELL_SIZE);
        self.write_u32(cell + CONST_SELF, self_ref);
        self.write_u32(cell + CONST_PREFIX, prefix);
        self.const_cells.insert(id, cell);
    }

    pub(super) fn string_desc(&mut self) -> u32 {
        self.desc(DescKey {
            kind: 1,
            tag: 0,
            enum_id: -1,
            name: "String".to_owned(),
            show: None,
            refs: Vec::new(),
            stride: 0,
        })
    }

    /// The descriptor of a list cell (`{head, tail}`), whose head is a reference or a word.
    pub(super) fn list_desc(&mut self, head_ref: bool) -> u32 {
        self.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name: "list".to_owned(),
            show: None,
            refs: vec![head_ref, true],
            stride: 0,
        })
    }

    pub(crate) fn desc(&mut self, key: DescKey) -> u32 {
        // `from_core` gives every constructed error enum a show function (its Show, or the
        // derived name when the type does not satisfy Show), and `build` maps it to a table
        // index, so a language-produced error's descriptor always has one. `show_failure`
        // relies on that.
        debug_assert!(
            key.enum_id < 0 || key.show.is_some(),
            "the error `{}` has no show function; `from_core` must give every constructed error \
             enum one (`Derive::ErrorName` when its type does not satisfy Show)",
            key.name
        );
        if let Some(&o) = self.descs.get(&key) {
            return o;
        }
        let o = self.alloc_static(if key.kind == DESC_KIND_ARRAY {
            DESC_SIZE + 4
        } else {
            DESC_SIZE
        });
        let refs = if key.refs.is_empty() {
            0
        } else {
            let r = self.alloc_static(key.refs.len() as u32);
            for (i, &b) in key.refs.iter().enumerate() {
                self.data[(r + i as u32) as usize] = u8::from(b);
            }
            r
        };
        let name = key.name.as_bytes().to_vec();
        let (name_ptr, name_len) = if name.is_empty() {
            (0, 0)
        } else {
            let p = self.alloc_static(name.len() as u32);
            self.data[p as usize..p as usize + name.len()].copy_from_slice(&name);
            (p, name.len() as u32)
        };
        self.write_u32(o + DESC_KIND, key.kind);
        self.write_u32(o + DESC_NSLOTS, key.refs.len() as u32);
        self.write_u32(o + DESC_TAG, key.tag);
        self.write_u32(o + DESC_ENUM, key.enum_id as u32);
        self.write_u32(o + DESC_REFS, refs);
        self.write_u32(o + DESC_SHOW, key.show.unwrap_or(u32::MAX));
        self.write_u32(o + DESC_NAME, name_ptr);
        self.write_u32(o + DESC_NAMELEN, name_len);
        if key.kind == DESC_KIND_ARRAY {
            self.write_u32(o + DESC_STRIDE, key.stride);
        }
        self.descs.insert(key, o);
        o
    }

    /// If `ty` is Prelude's `Option[A]`, the descriptor of `Some` (its field as a word), the
    /// static `None` object, and the representation of the `Some` field. The runtime's
    /// `Int.parse` and friends build the option with these.
    pub(crate) fn option_parts(&mut self, ty: TypeId) -> Option<(u32, u32, Repr)> {
        let core = self.core();
        let option = core.option?;
        let field = match core.ty(ty) {
            Type::Enum(id, args) if *id == option && args.len() == 1 => args[0],
            _ => return None,
        };
        let repr = self.repr(field);
        let (some, none) = self.option_consts(repr == Repr::Ref);
        Some((some, none, repr))
    }

    /// The descriptor of Prelude's `Some` whose field is a reference (`field_ref`) or another
    /// word, and the static `None` object (the same for every field).
    pub(crate) fn option_consts(&mut self, field_ref: bool) -> (u32, u32) {
        let some = self.desc(DescKey {
            kind: 0,
            tag: OPTION_SOME as u32,
            enum_id: -1,
            name: "Some".to_owned(),
            show: None,
            refs: vec![field_ref],
            stride: 0,
        });
        let none_desc = self.desc(DescKey {
            kind: 0,
            tag: OPTION_NONE as u32,
            enum_id: -1,
            name: "None".to_owned(),
            show: None,
            refs: Vec::new(),
            stride: 0,
        });
        let none = self.nullary(none_desc);
        (some, none)
    }

    /// The representation of the elements of the array type `ty` (`[api.std.array]`).
    pub(crate) fn array_elem_repr(&self, ty: TypeId) -> Repr {
        match self.core().ty(ty) {
            Type::Array(elem) => self.repr(*elem),
            _ => Repr::Ref,
        }
    }

    /// The layout of the element of the array type `ty` when it is an all-scalar record or tuple
    /// stored inline (E2), or `None` when the elements stay one-word values or references.
    ///
    /// This ignores the local-only exclusions of [`values::Values`]: an array element can be
    /// stored inline even when the same type is a function parameter, a result or a list head.
    pub(crate) fn inline_array_elem(&self, ty: TypeId) -> Option<values::Layout> {
        if !self.options.aggregate_arrays {
            return None;
        }
        match self.core().ty(ty) {
            Type::Array(elem) => values::all_scalar(self.core(), *elem),
            _ => None,
        }
    }

    /// The descriptor of the box made when an inline array element is read as a reference (E2).
    /// Its slots are the element's scalar leaves and it has no references, exactly what
    /// `Value::Make` gives a boxed all-scalar record or tuple.
    pub(crate) fn inline_array_elem_desc(&mut self, ty: TypeId) -> Option<u32> {
        let layout = self.inline_array_elem(ty)?;
        let slots = layout.slots();
        Some(self.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name: layout.name.clone(),
            show: None,
            refs: vec![false; slots],
            stride: 0,
        }))
    }

    /// The descriptor of an array of type `ty` (`runtime/array.rs`): kind 2, with `refs[0]`
    /// telling whether the elements are references and the element stride in slots.
    ///
    /// A scalar array keeps a stride of 1 (one 8-byte word). An array of an all-scalar record or
    /// tuple stores the element inline, one slot per scalar leaf, so `refs[0]` is false whatever
    /// the IR representation of the element is.
    pub(crate) fn array_desc(&mut self, ty: TypeId) -> u32 {
        let inline = self.inline_array_elem(ty);
        let stride = inline.as_ref().map_or(1, |layout| layout.slots() as u32);
        let elem_ref = inline.is_none() && self.array_elem_repr(ty) == Repr::Ref;
        self.desc(DescKey {
            kind: DESC_KIND_ARRAY,
            tag: 0,
            enum_id: -1,
            name: "Array".to_owned(),
            show: None,
            refs: vec![elem_ref],
            stride,
        })
    }

    /// A static object (rc == 0) of the descriptor, with no fields.
    pub(crate) fn nullary(&mut self, desc: u32) -> u32 {
        if let Some(&o) = self.nullary.get(&desc) {
            return o;
        }
        let o = self.alloc_static(HDR);
        self.write_u32(o + 8, desc);
        self.nullary.insert(desc, o);
        o
    }

    /// A static closure record (rc == 0) without captures.
    pub(crate) fn static_closure(&mut self, function: FnId) -> u32 {
        if let Some(&o) = self.static_closures.get(&function) {
            return o;
        }
        let desc = self.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name: check::display_name(&self.m.functions[function].name).to_owned(),
            show: None,
            refs: vec![false],
            stride: 0,
        });
        let o = self.alloc_static(HDR + 8);
        self.write_u32(o + 8, desc);
        let table = self.fn_table[ix(function)];
        self.write_u32(o + HDR, table);
        self.static_closures.insert(function, o);
        o
    }

    pub(crate) fn fn_index(&self, id: FnId) -> u32 {
        self.fn_index[ix(id)]
    }

    /// The type index of a call with the given parameter and result representations.
    pub(crate) fn call_ty(&mut self, params: &[Repr], ret: Repr) -> u32 {
        let params: Vec<ValType> = params.iter().map(|&r| val(r)).collect();
        let results: Vec<ValType> = repr_val(ret).into_iter().collect();
        self.ty(params, results)
    }

    /// The wasm value types the operands push, in order: one per scalar leaf of a value held by
    /// value, one for any other value, and none for `()`.
    ///
    /// The operands are the authority, not the declared function type: a lifted closure can take
    /// the evidence table as an ordinary last parameter (spec 8.4), and a `()` parameter has no
    /// representation.
    pub(super) fn operand_val_types(&self, cx: &FnCtx, args: &[Operand]) -> Vec<ValType> {
        let mut out = Vec::new();
        for arg in args {
            match arg {
                Operand::Var(v) => match cx.aggregate(*v) {
                    Some(n) => {
                        for i in 0..n {
                            out.push(val(cx.leaf_repr(*v, i)));
                        }
                    }
                    None => out.push(val(cx.f.vars[*v].repr)),
                },
                Operand::Const(c) => {
                    if c.repr() != Repr::Void {
                        out.push(val(c.repr()));
                    }
                }
            }
        }
        out
    }

    /// The wasm result types of a call returning `ty`, and the leaves to store when the result is
    /// held by value.
    pub(super) fn result_val_types(&self, ty: TypeId) -> (Vec<ValType>, Option<Vec<Repr>>) {
        match self.leaves(ty) {
            Some(leaves) => (
                leaves.iter().map(|&r| val(r)).collect(),
                Some(leaves.to_vec()),
            ),
            None => (repr_val(self.repr(ty)).into_iter().collect(), None),
        }
    }

    /// Stores a call's result: one wasm local per leaf of a value held by value, or the single
    /// representation of any other result.
    pub(super) fn assign_result(
        &mut self,
        cx: &mut FnCtx,
        ins: &mut InstructionSink,
        dst: Option<VarId>,
        ret: Repr,
        leaves: Option<Vec<Repr>>,
    ) {
        let Some(leaves) = leaves else {
            self.assign(cx, ins, dst, ret);
            return;
        };
        match dst {
            Some(d) => {
                if cx.aggregate(d) != Some(leaves.len() as u32) {
                    self.internal(format!(
                        "a result of {} has no destination in {}",
                        leaves.len(),
                        cx.f.name
                    ));
                    return;
                }
                // The leaves are on the stack in field order; store them in reverse.
                for i in (0..leaves.len() as u32).rev() {
                    ins.local_set(cx.leaf(d, i));
                }
            }
            // An unused result is given up like any other value: the leaves own no reference, so
            // each is dropped from the operand stack.
            None => {
                for _ in &leaves {
                    ins.drop();
                }
            }
        }
    }
}
