use super::*;
use crate::contract::auxiliary::ConstBridgePlan as Bridges;
pub(in crate::compiler::plan) trait Preparation: Sized {
    fn new(
        flow: &crate::contract::ir::Program,
        groups: &[crate::contract::lowered::ConstGroup],
        closures: &crate::contract::lowered::ClosureMetadata,
        layouts: &mut TypeLayouts<'_>,
    ) -> Result<Self, Vec<Error>>;
    fn collect(
        &mut self,
        source: TypeId,
        destination: TypeId,
        layouts: &mut TypeLayouts<'_>,
    ) -> Result<(), Vec<Error>>;
}
impl Preparation for Bridges {
    fn new(
        flow: &crate::contract::ir::Program,
        groups: &[crate::contract::lowered::ConstGroup],
        closures: &crate::contract::lowered::ClosureMetadata,
        layouts: &mut TypeLayouts<'_>,
    ) -> Result<Self, Vec<Error>> {
        let mut result = Self::default();
        let mut family = HashSet::new();
        for group in groups {
            for &typed in &group.specializations {
                result.getters.insert(typed, group.canonical);
                family.insert((group.canonical, typed));
            }
        }
        for group in flow.const_function_groups.clone() {
            for &source in &group {
                for &destination in &group {
                    family.insert((source, destination));
                }
            }
        }
        loop {
            let mut added = false;
            for (&source, &(parent, ordinal)) in &closures.families {
                for (&destination, &(typed_parent, typed_ordinal)) in &closures.families {
                    if ordinal == typed_ordinal && family.contains(&(parent, typed_parent)) {
                        added |= family.insert((source, destination));
                    }
                }
            }
            if !added {
                break;
            }
        }
        let mut references: HashMap<FnId, Vec<TypeId>> = HashMap::new();
        for (_, function) in flow.functions.iter() {
            for (_, statement) in function.stmts.iter() {
                if let Stmt::Let {
                    dst: Some(dst),
                    value: Value::FnRef(target),
                    ..
                } = statement
                {
                    let ty = function.vars[*dst].ty;
                    let types = references.entry(*target).or_default();
                    if !types.contains(&ty) {
                        types.push(ty);
                    }
                }
            }
        }
        for (source, destination) in family {
            for &source_ty in references.get(&source).into_iter().flatten() {
                for &destination_ty in references.get(&destination).into_iter().flatten() {
                    result
                        .callables
                        .push((source, destination, source_ty, destination_ty));
                }
            }
        }
        result.callables.sort_by_key(|(a, b, c, d)| {
            (
                u32::from(a.into_raw()),
                u32::from(b.into_raw()),
                c.index(),
                d.index(),
            )
        });
        for group in groups {
            for &typed in &group.specializations {
                result.collect(
                    flow.functions[group.canonical].ret,
                    flow.functions[typed].ret,
                    layouts,
                )?;
            }
        }
        Ok(result)
    }

    fn collect(
        &mut self,
        source: TypeId,
        destination: TypeId,
        layouts: &mut TypeLayouts<'_>,
    ) -> Result<(), Vec<Error>> {
        if self.ids.contains_key(&(source, destination)) {
            return Ok(());
        }
        let index = self.pairs.len();
        self.ids.insert((source, destination), index);
        self.pairs.push((source, destination));
        if source == destination {
            return Ok(());
        }
        let mut dependencies = Vec::new();
        match (
            layouts.shapes.types[source].clone(),
            layouts.shapes.types[destination].clone(),
        ) {
            (Type::Any, _) => layouts.add_any_box_type(destination),
            (_, Type::Any) => layouts.add_any_box_type(source),
            (Type::List(a), Type::List(b)) => dependencies.push((a, b)),
            (Type::Tuple(a), Type::Tuple(b)) if a.len() == b.len() => {
                dependencies.extend(a.into_iter().zip(b))
            }
            (Type::Enum(a, _), Type::Enum(b, _)) if a == b => {
                let source_fields = crate::semantics::abi::bridge_fields(layouts, source);
                let destination_fields = crate::semantics::abi::bridge_fields(layouts, destination);
                for (left, right) in source_fields.into_iter().zip(destination_fields) {
                    for (a, b) in left.into_iter().zip(right) {
                        if layouts.has_materialized_payload(a)
                            && layouts.has_materialized_payload(b)
                        {
                            dependencies.push((a, b));
                        }
                    }
                }
            }
            (Type::Fn(_), Type::Fn(_)) => {
                for &(a, b, at, bt) in &self.callables {
                    if at != source || bt != destination {
                        continue;
                    }
                    match (
                        layouts.functions.closure_functions.get(&a),
                        layouts.functions.closure_functions.get(&b),
                    ) {
                        (Some(&ai), Some(&bi)) => {
                            let ac = &layouts.functions.closure_environments[ai].captures;
                            let bc = &layouts.functions.closure_environments[bi].captures;
                            if ac.len() != bc.len() {
                                return Err(invalid_flow_ir(
                                    "shared const",
                                    "specialized closure capture arity differs",
                                ));
                            }
                            dependencies.extend(ac.iter().zip(bc).map(|(a, b)| (a.ty, b.ty)));
                        }
                        (None, None) => {}
                        _ => {
                            return Err(invalid_flow_ir(
                                "shared const",
                                "specialized closure capture representation differs",
                            ));
                        }
                    }
                }
            }
            _ => {
                return Err(unsupported(
                    "shared const",
                    "incompatible canonical and specialized value layouts",
                ));
            }
        }
        for (a, b) in dependencies {
            self.collect(a, b, layouts)?;
        }
        Ok(())
    }
}
