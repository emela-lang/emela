//! Read-only C ABI accessors. Final plans contain cached ownership and reachability properties.
use crate::contract::{abi::*, ir::*};
use crate::semantics::c::{function_type_name, type_name};
use crate::semantics::representations::dispatch as c_type;
use std::collections::HashSet;
pub(crate) trait LayoutRead: Sized {
    fn has_child_waits(&self) -> bool;
    fn has_error_notes(&self) -> bool;
    fn has_error_ownership(&self) -> bool;
    fn error_notes_kind(&self) -> u32;
    fn error_note_kind(&self) -> u32;
    fn enum_is_i32(&self, id: crate::contract::ir::EnumId) -> bool;
    fn enum_layout_required(&self, id: crate::contract::ir::EnumId) -> bool;
    fn type_repr(&self, ty: TypeId) -> Repr;
    fn c_type(&self, ty: TypeId) -> String;
    fn is_named_external(&self, ty: TypeId, name: &str) -> bool;
    fn is_runtime_handle(&self, ty: TypeId) -> bool;
    fn is_cause(&self, ty: TypeId) -> bool;
    fn error_enum(&self, ty: TypeId) -> Option<EnumId>;
    fn error_type(&self, enum_id: EnumId) -> Option<TypeId>;
    fn enum_fields(&self, ty: TypeId) -> Option<&[Vec<TypeId>]>;
    fn has_cell_cycles(&self) -> bool;
    fn cell_content_can_cycle(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool;
    fn cell_content_c_type(&self, content: TypeId) -> String;
    fn cell_kind(&self, cell: TypeId) -> usize;
    fn list_node_name(&self, ty: TypeId) -> String;
    fn list_index(&self, ty: TypeId) -> u32;
    fn is_managed_list(&self, ty: TypeId) -> bool;
    fn is_managed_value(&self, ty: TypeId) -> bool;
    fn contains_owned_ref(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool;
    fn contains_heap_ref(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool;
    fn is_recursive_box_field(&self, owner: TypeId, variant: usize, field: usize) -> bool;
    fn has_materialized_payload(&self, ty: TypeId) -> bool;
    fn recursive_box_kind(&self, ty: TypeId) -> Option<u32>;
    fn string_owner_kind(&self) -> u32;
    fn any_box_kind(&self) -> u32;
    fn closure_environment_kind(&self, index: usize) -> u32;
    fn aggregate_helper_index(&self, ty: TypeId) -> Option<usize>;
    fn runtime_value_kind_base(&self) -> u32;
    fn function_adapter_kind(&self, index: usize) -> u32;
    fn resume_kind(&self) -> u32;
    fn scope_kind(&self) -> u32;
    fn child_kind(&self) -> u32;
    fn deferred_kind(&self) -> u32;
    fn cause_kind(&self) -> u32;
    fn evidence_node_kind(&self) -> u32;
    fn handler_kind(&self, effect: EffectId) -> u32;
    fn error_declaration_dependencies(&self) -> HashSet<usize>;
    fn has_borrowed_views(&self, ty: TypeId) -> bool;
    fn has_byte_views(&self, ty: TypeId) -> bool;
}
impl LayoutRead for LayoutBuilder<'_> {
    fn has_child_waits(&self) -> bool {
        !self.runtime.child_join_types.is_empty()
            || !self.runtime.child_await_types.is_empty()
            || self.runtime.child_interrupts
    }

    fn has_error_notes(&self) -> bool {
        self.runtime.has_scopes && self.runtime.has_error_abi
    }

    fn has_error_ownership(&self) -> bool {
        self.has_error_notes() || self.runtime.managed_error_payloads
    }

    fn error_notes_kind(&self) -> u32 {
        self.cause_kind() + 1
    }

    fn error_note_kind(&self) -> u32 {
        self.cause_kind() + 2
    }

    fn enum_is_i32(&self, id: crate::contract::ir::EnumId) -> bool {
        let def = &self.shapes.core.enums[id];
        !def.error
            && !def.external
            && !def.variants.is_empty()
            && def.variants.iter().all(|variant| variant.fields.is_empty())
    }

    fn enum_layout_required(&self, id: crate::contract::ir::EnumId) -> bool {
        let def = &self.shapes.core.enums[id];
        def.name != "Std.Cause.Cause"
            && !def.external
            && !def.variants.is_empty()
            && !self.enum_is_i32(id)
    }

    fn type_repr(&self, ty: TypeId) -> Repr {
        if let Some(properties) = &self.properties {
            return properties.types[ty.index()].representation;
        }
        crate::semantics::type_representations::repr(self, &self.shapes.types[ty])
    }

    fn c_type(&self, ty: TypeId) -> String {
        match self.shapes.types[ty] {
            Type::Int | Type::Bool => "int32_t".to_owned(),
            Type::Int64 => "int64_t".to_owned(),
            Type::Float => "double".to_owned(),
            Type::Unit | Type::Never | Type::Set(_) => "void".to_owned(),
            Type::String => "EmelaString".to_owned(),
            Type::Bytes => "EmelaBytes".to_owned(),
            Type::Tuple(_) => type_name(ty),
            Type::Fn(_) => function_type_name(ty),
            Type::Enum(id, _) if self.enum_is_i32(id) => "int32_t".to_owned(),
            Type::Enum(id, _) if self.enum_layout_required(id) => {
                type_name(self.shapes.aggregates[self.shapes.aggregate_ids[&ty]].ty)
            }
            Type::Any => "EmelaAny".to_owned(),
            _ => c_type(self.type_repr(ty)).to_owned(),
        }
    }

    fn is_named_external(&self, ty: TypeId, name: &str) -> bool {
        matches ! (& self.shapes.types [ty] , Type :: Enum (id , _) if self.shapes.core . enums [* id] . external && self.shapes.core . enums [* id] . name == name)
    }

    fn is_runtime_handle(&self, ty: TypeId) -> bool {
        self.is_named_external(ty, "Std.Fiber.Fiber")
            || self.is_named_external(ty, "Std.Deferred.Deferred")
            || self.is_cause(ty)
    }

    fn is_cause(&self, ty: TypeId) -> bool {
        matches ! (& self.shapes.types [ty] , Type :: Enum (id , _) if self.shapes.core . enums [* id] . name == "Std.Cause.Cause")
    }

    fn error_enum(&self, ty: TypeId) -> Option<EnumId> {
        match self.shapes.types[ty] {
            Type::Enum(id, _) if self.shapes.core.enums[id].error => Some(id),
            _ => None,
        }
    }

    fn error_type(&self, enum_id: EnumId) -> Option<TypeId> {
        self.runtime
            .error_types
            .iter()
            .find_map(|(id, ty)| (*id == enum_id).then_some(*ty))
    }

    fn enum_fields(&self, ty: TypeId) -> Option<&[Vec<TypeId>]> {
        let index = *self.shapes.aggregate_ids.get(&ty)?;
        match &self.shapes.aggregates[index].kind {
            AggregateKind::Enum(variants) => Some(variants),
            AggregateKind::Tuple(_) => None,
        }
    }

    fn has_cell_cycles(&self) -> bool {
        if let Some(properties) = &self.properties {
            return properties.cell_cycles;
        }
        self.shapes.cell_types.iter().any(|&ty| {
            let Type::Cell(content) = self.shapes.types[ty] else {
                unreachable!()
            };
            self.cell_content_can_cycle(content, &mut HashSet::new())
        })
    }

    fn cell_content_can_cycle(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        if let Some(properties) = &self.properties {
            return properties.types[ty.index()].cell_cycles;
        }
        if !seen.insert(ty) {
            return false;
        }
        if self.is_runtime_handle(ty) {
            return true;
        }
        match self.shapes.types[ty].clone() {
            Type::Any
            | Type::Cell(_)
            | Type::Fn(_)
            | Type::Handler(_)
            | Type::Evidence
            | Type::FiberScope => true,
            Type::List(element) => self.cell_content_can_cycle(element, seen),
            _ => self.shapes.aggregate_ids.get(&ty).is_some_and(|&index| {
                let fields = match &self.shapes.aggregates[index].kind {
                    AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                    AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
                };
                fields
                    .into_iter()
                    .flatten()
                    .any(|&field| self.cell_content_can_cycle(field, seen))
            }),
        }
    }

    fn cell_content_c_type(&self, content: TypeId) -> String {
        if self.type_repr(content) == Repr::Void {
            "uint8_t".into()
        } else {
            self.c_type(content)
        }
    }

    fn cell_kind(&self, cell: TypeId) -> usize {
        self.shapes.list_types.len()
            + self.shapes.recursive_box_types.len()
            + self.shapes.cell_ids[&cell]
    }

    fn list_node_name(&self, ty: TypeId) -> String {
        format!("emela_list_node_{}", self.shapes.list_ids[&ty])
    }

    fn list_index(&self, ty: TypeId) -> u32 {
        u32::try_from(self.shapes.list_ids[&ty]).expect("too many concrete List layouts")
    }

    fn is_managed_list(&self, ty: TypeId) -> bool {
        matches!(self.shapes.types[ty], Type::List(_))
    }

    fn is_managed_value(&self, ty: TypeId) -> bool {
        if let Some(properties) = &self.properties {
            return properties.types[ty.index()].owned;
        }
        self.contains_owned_ref(ty, &mut HashSet::new())
    }

    fn contains_owned_ref(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        if let Some(properties) = &self.properties {
            return properties.types[ty.index()].owned;
        }
        if self.is_runtime_handle(ty) {
            return true;
        }
        if (self.runtime.any_box_required || self.has_error_ownership())
            && matches!(self.shapes.types[ty], Type::Any)
        {
            return true;
        }
        if matches!(
            self.shapes.types[ty],
            Type::String
                | Type::Bytes
                | Type::List(_)
                | Type::Cell(_)
                | Type::Handler(_)
                | Type::Evidence
                | Type::FiberScope
        ) {
            return true;
        }
        if matches!(self.shapes.types[ty], Type::Fn(_)) {
            return self.shapes.closure_function_types.contains(&ty);
        }
        if !seen.insert(ty) {
            return false;
        }
        let Some(&aggregate_index) = self.shapes.aggregate_ids.get(&ty) else {
            return false;
        };
        let aggregate = &self.shapes.aggregates[aggregate_index];
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => fields.iter().enumerate().any(|(index, &field)| {
                self.shapes.recursive_edges.contains(&(ty, 0, index))
                    || self.contains_owned_ref(field, seen)
            }),
            AggregateKind::Enum(variants) => {
                variants.iter().enumerate().any(|(variant, fields)| {
                    fields.iter().enumerate().any(|(index, &field)| {
                        self.shapes.recursive_edges.contains(&(ty, variant, index))
                            || self.contains_owned_ref(field, seen)
                    })
                })
            }
        }
    }

    fn contains_heap_ref(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        if let Some(properties) = &self.properties {
            return properties.types[ty.index()].heap;
        }
        if self.is_runtime_handle(ty) {
            return true;
        }
        if (self.runtime.any_box_required || self.has_error_ownership())
            && matches!(self.shapes.types[ty], Type::Any)
        {
            return true;
        }
        if matches!(
            self.shapes.types[ty],
            Type::List(_) | Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
        ) {
            return true;
        }
        if matches!(self.shapes.types[ty], Type::Fn(_)) {
            return self.shapes.closure_function_types.contains(&ty);
        }
        if !seen.insert(ty) {
            return false;
        }
        let Some(&aggregate_index) = self.shapes.aggregate_ids.get(&ty) else {
            return false;
        };
        let aggregate = &self.shapes.aggregates[aggregate_index];
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => fields.iter().enumerate().any(|(index, &field)| {
                self.shapes.recursive_edges.contains(&(ty, 0, index))
                    || self.contains_heap_ref(field, seen)
            }),
            AggregateKind::Enum(variants) => {
                variants.iter().enumerate().any(|(variant, fields)| {
                    fields.iter().enumerate().any(|(index, &field)| {
                        self.shapes.recursive_edges.contains(&(ty, variant, index))
                            || self.contains_heap_ref(field, seen)
                    })
                })
            }
        }
    }

    fn is_recursive_box_field(&self, owner: TypeId, variant: usize, field: usize) -> bool {
        self.shapes
            .recursive_edges
            .contains(&(owner, variant, field))
    }

    fn has_materialized_payload(&self, ty: TypeId) -> bool {
        match self.shapes.types[ty] {
            Type::Enum(id, _) if self.enum_layout_required(id) => {
                self.shapes.aggregate_ids.contains_key(&ty)
            }
            Type::Tuple(_) => self.shapes.aggregate_ids.contains_key(&ty),
            Type::Fn(_) => self.shapes.function_types.contains(&ty),
            Type::List(_) => self.shapes.list_ids.contains_key(&ty),
            Type::Cell(_) => self.shapes.cell_ids.contains_key(&ty),
            Type::Any => self.runtime.any_abi,
            _ => true,
        }
    }

    fn recursive_box_kind(&self, ty: TypeId) -> Option<u32> {
        let index = *self.shapes.recursive_box_ids.get(&ty)?;
        u32::try_from(self.shapes.list_types.len() + index).ok()
    }

    fn string_owner_kind(&self) -> u32 {
        u32::try_from(
            self.shapes.list_types.len()
                + self.shapes.recursive_box_types.len()
                + self.shapes.cell_types.len(),
        )
        .expect("too many fixed-heap object kinds")
    }

    fn any_box_kind(&self) -> u32 {
        self.runtime_value_kind_base()
            + u32::try_from(self.shapes.function_adapters.len())
                .expect("too many function adapters")
            + u32::from(self.runtime.evidence_nodes_required)
            + u32::try_from(self.shapes.handler_layouts.len()).expect("too many handler effects")
    }

    fn closure_environment_kind(&self, index: usize) -> u32 {
        u32::try_from(self.string_owner_kind() as usize + 1 + index)
            .expect("too many fixed-heap object kinds")
    }

    fn aggregate_helper_index(&self, ty: TypeId) -> Option<usize> {
        let index = *self.shapes.aggregate_ids.get(&ty)?;
        self.is_managed_value(ty).then_some(index)
    }

    fn runtime_value_kind_base(&self) -> u32 {
        if self.functions.closure_environments.is_empty() {
            self.string_owner_kind() + 1
        } else {
            self.closure_environment_kind(self.functions.closure_environments.len() - 1) + 1
        }
    }

    fn function_adapter_kind(&self, index: usize) -> u32 {
        self.runtime_value_kind_base() + u32::try_from(index).expect("too many function adapters")
    }

    fn resume_kind(&self) -> u32 {
        self.any_box_kind() + 1
    }

    fn scope_kind(&self) -> u32 {
        self.resume_kind() + 1
    }

    fn child_kind(&self) -> u32 {
        self.resume_kind() + 2
    }

    fn deferred_kind(&self) -> u32 {
        self.resume_kind() + 3
    }

    fn cause_kind(&self) -> u32 {
        self.resume_kind() + 4
    }

    fn evidence_node_kind(&self) -> u32 {
        self.runtime_value_kind_base()
            + u32::try_from(self.shapes.function_adapters.len())
                .expect("too many function adapters")
    }

    fn handler_kind(&self, effect: EffectId) -> u32 {
        let handler_index = self.shapes.handler_layout_ids[&effect];
        self.runtime_value_kind_base()
            + u32::try_from(self.shapes.function_adapters.len())
                .expect("too many function adapters")
            + u32::from(self.runtime.evidence_nodes_required)
            + u32::try_from(handler_index).expect("too many handler effects")
    }

    fn error_declaration_dependencies(&self) -> HashSet<usize> {
        if let Some(properties) = &self.properties {
            return properties.error_declarations.clone();
        }
        let mut pending: Vec<_> = self.runtime.error_types.iter().map(|&(_, ty)| ty).collect();
        let mut visited = HashSet::new();
        while let Some(ty) = pending.pop() {
            let Some(&index) = self.shapes.aggregate_ids.get(&ty) else {
                continue;
            };
            if !visited.insert(index) {
                continue;
            }
            let aggregate = &self.shapes.aggregates[index];
            let variants = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in variants.into_iter().enumerate() {
                for (field, &ty) in fields.iter().enumerate() {
                    if !self.is_recursive_box_field(aggregate.ty, variant, field) {
                        pending.push(ty);
                    }
                }
            }
        }
        visited
    }

    fn has_byte_views(&self, ty: TypeId) -> bool {
        if matches!(self.shapes.types[ty], Type::Bytes) {
            return true;
        }
        let Some(&index) = self.shapes.aggregate_ids.get(&ty) else {
            return false;
        };
        let fields = match &self.shapes.aggregates[index].kind {
            AggregateKind::Tuple(fields) => vec![fields.as_slice()],
            AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
        };
        fields.iter().enumerate().any(|(variant, fields)| {
            fields.iter().enumerate().any(|(field, &field_ty)| {
                !self.is_recursive_box_field(ty, variant, field) && self.has_byte_views(field_ty)
            })
        })
    }

    fn has_borrowed_views(&self, ty: TypeId) -> bool {
        if let Some(properties) = &self.properties {
            return properties.types[ty.index()].borrowed_views;
        }
        if matches!(self.shapes.types[ty], Type::String | Type::Bytes) {
            return true;
        }
        let Some(&index) = self.shapes.aggregate_ids.get(&ty) else {
            return false;
        };
        let fields = match &self.shapes.aggregates[index].kind {
            AggregateKind::Tuple(fields) => vec![fields.as_slice()],
            AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
        };
        fields.iter().enumerate().any(|(variant, fields)| {
            fields.iter().enumerate().any(|(field, &field_ty)| {
                !self.is_recursive_box_field(ty, variant, field)
                    && self.has_borrowed_views(field_ty)
            })
        })
    }
}

pub(crate) fn handler_type_name(effect: EffectId) -> String {
    format!("emela_handler_{}", u32::from(effect.into_raw()))
}

pub(crate) fn option_payload_type(layouts: &LayoutBuilder<'_>, ty: TypeId) -> Option<TypeId> {
    let Type::Enum(enum_id, _) = layouts.shapes.types[ty] else {
        return None;
    };
    let definition = &layouts.shapes.core.enums[enum_id];
    if definition.name != "Option" {
        return None;
    }
    let some_index = definition
        .variants
        .iter()
        .position(|variant| variant.name == "Some")?;
    let fields = layouts.enum_fields(ty)?.get(some_index)?;
    (fields.len() == 1).then_some(fields[0])
}

pub(crate) fn scalar(types: &TypeTable, ty: TypeId) -> bool {
    matches!(
        types[ty],
        Type::Int | Type::Int64 | Type::Float | Type::Bool | Type::Unit | Type::Never
    )
}

pub(crate) fn bridge_fields(layouts: &LayoutBuilder<'_>, ty: TypeId) -> Vec<Vec<TypeId>> {
    let Some(&index) = layouts.shapes.aggregate_ids.get(&ty) else {
        return vec![];
    };
    match &layouts.shapes.aggregates[index].kind {
        AggregateKind::Tuple(fields) => vec![fields.clone()],
        AggregateKind::Enum(fields) => fields.clone(),
    }
}
