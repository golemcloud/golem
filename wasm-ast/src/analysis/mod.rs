// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod model;
pub use model::*;

/// Protobuf representation of analysis results
#[cfg(feature = "protobuf")]
pub mod protobuf;

/// Wave format support for types.
///
/// This module is optional and can be enabled with the `metadata` feature flag. It is enabled by default.
#[cfg(feature = "wave")]
pub mod wave;

use crate::component::*;
use crate::core::Mem;
use crate::AstCustomization;
use mappable_rc::Mrc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

pub type AnalysisResult<A> = Result<A, AnalysisFailure>;

#[derive(Debug, Clone)]
struct ComponentStackItem<Ast: AstCustomization + 'static> {
    component: Mrc<Component<Ast>>,
    component_idx: Option<ComponentIdx>,
}

type ResourceIdMap = HashMap<(Vec<ComponentIdx>, ComponentTypeIdx), AnalysedResourceId>;

#[derive(Debug, Clone)]
pub struct AnalysisContext<Ast: AstCustomization + 'static> {
    component_stack: Vec<ComponentStackItem<Ast>>,
    warnings: Rc<RefCell<Vec<AnalysisWarning>>>,
    resource_ids: Rc<RefCell<ResourceIdMap>>,
}

impl<Ast: AstCustomization + 'static> AnalysisContext<Ast> {
    pub fn new(component: Component<Ast>) -> AnalysisContext<Ast> {
        AnalysisContext::from_rc(Mrc::new(component))
    }

    /// Initializes an analyzer for a given component
    pub fn from_rc(component: Mrc<Component<Ast>>) -> AnalysisContext<Ast> {
        AnalysisContext {
            component_stack: vec![ComponentStackItem {
                component,
                component_idx: None,
            }],
            warnings: Rc::new(RefCell::new(Vec::new())),
            resource_ids: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// Get all top-level exports from the component with all the type information gathered from
    /// the component AST.
    pub fn get_top_level_exports(&self) -> AnalysisResult<Vec<AnalysedExport>> {
        let component = self.get_component();
        let mut result = Vec::new();
        for export in component.exports() {
            match export.kind {
                ComponentExternalKind::Func => {
                    let export = self.analyse_func_export(export.name.as_string(), export.idx)?;
                    result.push(AnalysedExport::Function(export));
                }
                ComponentExternalKind::Instance => {
                    let instance =
                        self.analyse_instance_export(export.name.as_string(), export.idx)?;
                    result.push(AnalysedExport::Instance(instance));
                }
                _ => self.warning(AnalysisWarning::UnsupportedExport(
                    UnsupportedExportWarning {
                        kind: export.kind.clone(),
                        name: export.name.as_string(),
                    },
                )),
            }
        }

        Ok(result)
    }

    /// Gets all the memories (not just the exported ones) from all modules within the WASM component
    pub fn get_all_memories(&self) -> AnalysisResult<Vec<Mem>> {
        let mut result = Vec::new();

        let mut component_stack = vec![self.get_component()];
        while let Some(component) = component_stack.pop() {
            for module in component.modules() {
                for mem in module.mems() {
                    result.push((*mem).clone());
                }
            }
            for inner_component in component.components() {
                component_stack.push(inner_component.clone());
            }
        }
        Ok(result)
    }

    pub fn warnings(&self) -> Vec<AnalysisWarning> {
        self.warnings.borrow().clone()
    }

    fn warning(&self, warning: AnalysisWarning) {
        self.warnings.borrow_mut().push(warning);
    }

    fn get_resource_id(&self, type_idx: &ComponentTypeIdx) -> AnalysedResourceId {
        let new_unique_id = self.resource_ids.borrow().len() as u64;
        let mut resource_ids = self.resource_ids.borrow_mut();
        let path = self
            .component_stack
            .iter()
            .filter_map(|item| item.component_idx)
            .collect();
        let key = (path, *type_idx);
        resource_ids
            .entry(key)
            .or_insert_with(|| {
                AnalysedResourceId(new_unique_id) // We don't to associate all IDs in each component, so this simple method can always generate a unique one
            })
            .clone()
    }

    fn analyse_func_export(
        &self,
        name: String,
        idx: ComponentFuncIdx,
    ) -> AnalysisResult<AnalysedFunction> {
        let (function_section, next_ctx) = self
            .get_final_referenced(format!("component function {idx}"), |component| {
                component.get_component_func(idx)
            })?;
        let (func_type_section, next_ctx) = match &*function_section {
            ComponentSection::Canon(Canon::Lift { function_type, .. }) => next_ctx
                .get_final_referenced(
                    format!("component function type {function_type}"),
                    |component| component.get_component_type(*function_type),
                ),
            ComponentSection::Import(ComponentImport {
                desc: ComponentTypeRef::Func(func_type_idx),
                ..
            }) => next_ctx.get_final_referenced(
                format!("component function type {func_type_idx}"),
                |component| component.get_component_type(*func_type_idx),
            ),
            ComponentSection::Import(ComponentImport { desc, .. }) => Err(AnalysisFailure::failed(
                format!("Expected function import, but got {:?} instead", desc),
            )),
            _ => Err(AnalysisFailure::failed(format!(
                "Expected canonical lift function or function import, but got {} instead",
                function_section.type_name()
            ))),
        }?;
        match &*func_type_section {
            ComponentSection::Type(ComponentType::Func(func_type)) => {
                next_ctx.analyse_component_func_type(name, func_type)
            }
            _ => Err(AnalysisFailure::failed(format!(
                "Expected function type, but got {} instead",
                func_type_section.type_name()
            ))),
        }
    }

    fn analyse_component_func_type(
        &self,
        name: String,
        func_type: &ComponentFuncType,
    ) -> AnalysisResult<AnalysedFunction> {
        let mut params: Vec<AnalysedFunctionParameter> = Vec::new();
        for (param_name, param_type) in &func_type.params {
            params.push(AnalysedFunctionParameter {
                name: param_name.clone(),
                typ: self.analyse_component_val_type(param_type)?,
            })
        }

        let mut results: Vec<AnalysedFunctionResult> = Vec::new();
        match &func_type.result {
            ComponentFuncResult::Unnamed(tpe) => {
                results.push(AnalysedFunctionResult {
                    name: None,
                    typ: self.analyse_component_val_type(tpe)?,
                });
            }
            ComponentFuncResult::Named(name_type_pairs) => {
                for (result_name, result_type) in name_type_pairs {
                    results.push(AnalysedFunctionResult {
                        name: Some(result_name.clone()),
                        typ: self.analyse_component_val_type(result_type)?,
                    })
                }
            }
        }

        Ok(AnalysedFunction {
            name,
            parameters: params,
            results,
        })
    }

    fn analyse_component_type_idx(
        &self,
        component_type_idx: &ComponentTypeIdx,
        analysed_resource_mode: Option<AnalysedResourceMode>,
    ) -> AnalysisResult<AnalysedType> {
        let (component_type_section, next_ctx) = self.get_final_referenced(
            format!("component type {component_type_idx}"),
            |component| component.get_component_type(*component_type_idx),
        )?;
        match &*component_type_section {
            ComponentSection::Type(ComponentType::Defined(component_defined_type)) => {
                next_ctx.analyse_component_defined_type(component_defined_type)
            }
            ComponentSection::Type(ComponentType::Func(_)) => Err(AnalysisFailure::failed(
                "Passing functions in exported functions is not supported",
            )),
            ComponentSection::Type(ComponentType::Component(_)) => Err(AnalysisFailure::failed(
                "Passing components in exported functions is not supported",
            )),
            ComponentSection::Type(ComponentType::Instance(_)) => Err(AnalysisFailure::failed(
                "Passing instances in exported functions is not supported",
            )),
            ComponentSection::Type(ComponentType::Resource { .. }) => Err(AnalysisFailure::failed(
                "Passing resources in exported functions is not supported",
            )),
            ComponentSection::Import(ComponentImport { desc, .. }) => match desc {
                ComponentTypeRef::Type(TypeBounds::Eq(component_type_idx)) => {
                    self.analyse_component_type_idx(component_type_idx, analysed_resource_mode)
                }
                ComponentTypeRef::Type(TypeBounds::SubResource) => {
                    match analysed_resource_mode {
                        Some(resource_mode) => {
                            let id = next_ctx.get_resource_id(component_type_idx);
                            Ok(AnalysedType::Handle(TypeHandle {
                                resource_id: id,
                                mode: resource_mode,
                            }))
                        }
                        None => Err(AnalysisFailure::failed("Reached a sub-resource type bound without a surrounding borrowed/owned resource type")),
                    }
                }
                _ => Err(AnalysisFailure::failed(format!(
                    "Imports {desc:?} is not supported as a defined type"
                ))),
            },
            _ => Err(AnalysisFailure::failed(format!(
                "Expected component type, but got {} instead",
                component_type_section.type_name()
            ))),
        }
    }

    fn analyse_component_val_type(&self, tpe: &ComponentValType) -> AnalysisResult<AnalysedType> {
        match tpe {
            ComponentValType::Primitive(primitive_value_type) => Ok(primitive_value_type.into()),
            ComponentValType::Defined(component_type_idx) => {
                self.analyse_component_type_idx(component_type_idx, None)
            }
        }
    }

    fn analyse_component_defined_type(
        &self,
        defined_type: &ComponentDefinedType,
    ) -> AnalysisResult<AnalysedType> {
        match defined_type {
            ComponentDefinedType::Primitive { typ } => Ok(typ.into()),
            ComponentDefinedType::Record { fields } => {
                let mut result = Vec::new();
                for (name, typ) in fields {
                    result.push(NameTypePair {
                        name: name.clone(),
                        typ: self.analyse_component_val_type(typ)?,
                    });
                }
                Ok(AnalysedType::Record(TypeRecord { fields: result }))
            }
            ComponentDefinedType::Variant { cases } => {
                let mut result = Vec::new();
                for case in cases {
                    result.push(NameOptionTypePair {
                        name: case.name.clone(),
                        typ: case
                            .typ
                            .as_ref()
                            .map(|t| self.analyse_component_val_type(t))
                            .transpose()?,
                    });
                }
                Ok(AnalysedType::Variant(TypeVariant { cases: result }))
            }
            ComponentDefinedType::List { elem } => Ok(AnalysedType::List(TypeList {
                inner: Box::new(self.analyse_component_val_type(elem)?),
            })),
            ComponentDefinedType::Tuple { elems } => {
                let mut result = Vec::new();
                for elem in elems {
                    result.push(self.analyse_component_val_type(elem)?);
                }
                Ok(AnalysedType::Tuple(TypeTuple { items: result }))
            }
            ComponentDefinedType::Flags { names } => Ok(AnalysedType::Flags(TypeFlags {
                names: names.clone(),
            })),
            ComponentDefinedType::Enum { names } => Ok(AnalysedType::Enum(TypeEnum {
                cases: names.clone(),
            })),
            ComponentDefinedType::Option { typ } => Ok(AnalysedType::Option(TypeOption {
                inner: Box::new(self.analyse_component_val_type(typ)?),
            })),
            ComponentDefinedType::Result { ok, err } => Ok(AnalysedType::Result(TypeResult {
                ok: ok
                    .as_ref()
                    .map(|t| self.analyse_component_val_type(t).map(Box::new))
                    .transpose()?,
                err: err
                    .as_ref()
                    .map(|t| self.analyse_component_val_type(t).map(Box::new))
                    .transpose()?,
            })),
            ComponentDefinedType::Owned { type_idx } => {
                self.analyse_component_type_idx(type_idx, Some(AnalysedResourceMode::Owned))
            }
            ComponentDefinedType::Borrowed { type_idx } => {
                self.analyse_component_type_idx(type_idx, Some(AnalysedResourceMode::Borrowed))
            }
        }
    }

    fn analyse_instance_export(
        &self,
        name: String,
        idx: InstanceIdx,
    ) -> AnalysisResult<AnalysedInstance> {
        let (instance_section, next_ctx) = self
            .get_final_referenced(format!("instance {idx}"), |component| {
                component.get_instance_wrapped(idx)
            })?;
        match &*instance_section {
            ComponentSection::Instance(instance) => match instance {
                ComponentInstance::Instantiate { component_idx, .. } => {
                    let (component_section, next_ctx) = next_ctx.get_final_referenced(
                        format!("component {component_idx}"),
                        |component| component.get_component(*component_idx),
                    )?;

                    match &*component_section {
                        ComponentSection::Component(referenced_component) => {
                            let next_ctx = next_ctx.push_component(
                                Mrc::map(component_section.clone(), |c| c.as_component()),
                                *component_idx,
                            );
                            let mut funcs = Vec::new();
                            for export in referenced_component.exports() {
                                match export.kind {
                                    ComponentExternalKind::Func => {
                                        let func = next_ctx.analyse_func_export(
                                            export.name.as_string(),
                                            export.idx,
                                        )?;
                                        funcs.push(func);
                                    }
                                    _ => next_ctx.warning(AnalysisWarning::UnsupportedExport(
                                        UnsupportedExportWarning {
                                            kind: export.kind.clone(),
                                            name: export.name.as_string(),
                                        },
                                    )),
                                }
                            }

                            Ok(AnalysedInstance {
                                name,
                                functions: funcs,
                            })
                        }
                        _ => Err(AnalysisFailure::failed(format!(
                            "Expected component, but got {} instead",
                            component_section.type_name()
                        ))),
                    }
                }
                ComponentInstance::FromExports { .. } => Err(AnalysisFailure::failed(
                    "Instance defined directly from exports are not supported",
                )),
            },
            _ => Err(AnalysisFailure::failed(format!(
                "Expected instance, but got {} instead",
                instance_section.type_name()
            ))),
        }
    }

    fn get_component(&self) -> Mrc<Component<Ast>> {
        self.component_stack.last().unwrap().component.clone()
    }

    fn get_components_from_stack(&self, count: u32) -> Vec<ComponentStackItem<Ast>> {
        self.component_stack
            .iter()
            .skip(self.component_stack.len() - count as usize - 1)
            .cloned()
            .collect()
    }

    fn push_component(
        &self,
        component: Mrc<Component<Ast>>,
        component_idx: ComponentIdx,
    ) -> AnalysisContext<Ast> {
        let mut component_stack = self.component_stack.clone();
        component_stack.push(ComponentStackItem {
            component,
            component_idx: Some(component_idx),
        });
        self.with_component_stack(component_stack)
    }

    fn with_component_stack(
        &self,
        component_stack: Vec<ComponentStackItem<Ast>>,
    ) -> AnalysisContext<Ast> {
        AnalysisContext {
            component_stack,
            warnings: self.warnings.clone(),
            resource_ids: self.resource_ids.clone(),
        }
    }

    fn get_final_referenced<F>(
        &self,
        description: impl AsRef<str>,
        f: F,
    ) -> AnalysisResult<(Mrc<ComponentSection<Ast>>, AnalysisContext<Ast>)>
    where
        F: Fn(&Component<Ast>) -> Option<Mrc<ComponentSection<Ast>>>,
    {
        let component = self.get_component();
        let direct_section = AnalysisFailure::fail_on_missing(f(&component), description)?;
        self.follow_redirects(direct_section)
    }

    fn follow_redirects(
        &self,
        section: Mrc<ComponentSection<Ast>>,
    ) -> AnalysisResult<(Mrc<ComponentSection<Ast>>, AnalysisContext<Ast>)> {
        let component = self.get_component();
        match &*section {
            ComponentSection::Export(ComponentExport { kind, idx, .. }) => {
                let next = match kind {
                    ComponentExternalKind::Module => AnalysisFailure::fail_on_missing(
                        component.get_module(*idx),
                        format!("module {idx}"),
                    )?,
                    ComponentExternalKind::Func => AnalysisFailure::fail_on_missing(
                        component.get_component_func(*idx),
                        format!("function {idx}"),
                    )?,
                    ComponentExternalKind::Value => AnalysisFailure::fail_on_missing(
                        component.get_value(*idx),
                        format!("value {idx}"),
                    )?,
                    ComponentExternalKind::Type => AnalysisFailure::fail_on_missing(
                        component.get_component_type(*idx),
                        format!("type {idx}"),
                    )?,
                    ComponentExternalKind::Instance => AnalysisFailure::fail_on_missing(
                        component.get_instance_wrapped(*idx),
                        format!("instance {idx}"),
                    )?,
                    ComponentExternalKind::Component => AnalysisFailure::fail_on_missing(
                        component.get_component(*idx),
                        format!("component {idx}"),
                    )?,
                };
                self.follow_redirects(next)
            }
            ComponentSection::Alias(Alias::InstanceExport {
                instance_idx, name, ..
            }) => {
                let (instance_section, next_ctx) = self
                    .get_final_referenced(format!("instance {instance_idx}"), |component| {
                        component.get_instance_wrapped(*instance_idx)
                    })?;
                next_ctx.find_instance_export(instance_section, name)
            }
            ComponentSection::Import(ComponentImport {
                desc: ComponentTypeRef::Type(TypeBounds::Eq(idx)),
                ..
            }) => {
                let maybe_tpe = component.get_component_type(*idx);
                let tpe = AnalysisFailure::fail_on_missing(maybe_tpe, format!("type {idx}"))?;
                self.follow_redirects(tpe)
            }
            ComponentSection::Alias(Alias::Outer {
                kind,
                target: AliasTarget { count, index },
            }) => {
                let referenced_components = self.get_components_from_stack(*count);
                let referenced_component = referenced_components
                    .first()
                    .ok_or(AnalysisFailure::failed(format!(
                        "Component stack underflow (count={count}, size={}",
                        self.component_stack.len()
                    )))?
                    .component
                    .clone();
                match kind {
                    OuterAliasKind::CoreModule => Err(AnalysisFailure::failed(
                        "Core module aliases are not supported",
                    )),
                    OuterAliasKind::CoreType => Err(AnalysisFailure::failed(
                        "Core type aliases are not supported",
                    )),
                    OuterAliasKind::Type => {
                        let maybe_tpe = referenced_component.get_component_type(*index);
                        let tpe =
                            AnalysisFailure::fail_on_missing(maybe_tpe, format!("type {index}"))?;
                        let next_ctx = self.with_component_stack(referenced_components);
                        next_ctx.follow_redirects(tpe)
                    }
                    OuterAliasKind::Component => {
                        let maybe_component = referenced_component.get_component(*index);
                        let component = AnalysisFailure::fail_on_missing(
                            maybe_component,
                            format!("component {index}"),
                        )?;
                        let next_ctx = self.with_component_stack(referenced_components);
                        next_ctx.follow_redirects(component)
                    }
                }
            }
            // TODO: support other redirections if needed
            _ => Ok((section, self.clone())),
        }
    }

    fn find_instance_export(
        &self,
        instance_section: Mrc<ComponentSection<Ast>>,
        name: &String,
    ) -> AnalysisResult<(Mrc<ComponentSection<Ast>>, AnalysisContext<Ast>)> {
        match &*instance_section {
            ComponentSection::Instance(_) => {
                let instance = Mrc::map(instance_section, |s| s.as_instance());
                let (maybe_export, next_ctx) = self.find_export_by_name(&instance, name)?;
                let export = AnalysisFailure::fail_on_missing(
                    maybe_export,
                    format!("missing aliased instance export {name} from instance"),
                )?;
                let wrapped = Mrc::new(ComponentSection::Export((*export).clone()));
                next_ctx.follow_redirects(wrapped)
            }
            ComponentSection::Import(ComponentImport {
                desc: ComponentTypeRef::Instance(type_idx),
                ..
            }) => {
                let maybe_tpe = self.get_component().get_component_type(*type_idx);
                let tpe = AnalysisFailure::fail_on_missing(maybe_tpe, format!("type {type_idx}"))?;
                let (tpe, next_ctx) = self.follow_redirects(tpe)?;
                next_ctx.find_instance_export(tpe, name)
            }
            ComponentSection::Type(ComponentType::Instance(decls)) => {
                match decls.find_export(name) {
                    Some(decl) => {
                        match decl {
                            ComponentTypeRef::Module(type_idx) => {
                                let maybe_tpe = self.get_component().get_component_type(*type_idx);
                                let tpe = AnalysisFailure::fail_on_missing(
                                    maybe_tpe,
                                    format!("type {type_idx}"),
                                )?;
                                self.follow_redirects(tpe)
                            }
                            ComponentTypeRef::Func(type_idx) => {
                                let maybe_tpe = self.get_component().get_component_type(*type_idx);
                                let tpe = AnalysisFailure::fail_on_missing(
                                    maybe_tpe,
                                    format!("type {type_idx}"),
                                )?;
                                self.follow_redirects(tpe)
                            }
                            ComponentTypeRef::Val(_val_type) => {
                                todo!()
                            }
                            ComponentTypeRef::Type(type_bounds) => {
                                match type_bounds {
                                    TypeBounds::Eq(component_type_idx) => {
                                        let decl = decls.get_component_type(*component_type_idx);
                                        let decl = AnalysisFailure::fail_on_missing(decl, format!("type {component_type_idx}"))?;

                                        match decl {
                                            InstanceTypeDeclaration::Core(_) => {
                                                Err(AnalysisFailure::failed("Core type aliases are not supported"))
                                            }
                                            InstanceTypeDeclaration::Type(component_type) => {
                                                Ok((Mrc::new(ComponentSection::Type(component_type.clone())), self.clone()))
                                            }
                                            InstanceTypeDeclaration::Alias(alias) => {
                                                let component_idx = self.component_stack.last().unwrap().component_idx.unwrap();
                                                let new_ctx = self.push_component(self.get_component(), component_idx);
                                                // Emulating an inner scope by duplicating the current component on the stack (TODO: refactor this)
                                                new_ctx.follow_redirects(Mrc::new(ComponentSection::Alias(alias.clone())))
                                            }
                                            InstanceTypeDeclaration::Export { .. } => {
                                                todo!()
                                            }
                                        }
                                    }
                                    TypeBounds::SubResource => {
                                        Err(AnalysisFailure::failed("Reached a sub-resource type bound without a surrounding borrowed/owned resource type in find_instance_export"))
                                    }
                                }
                            }
                            ComponentTypeRef::Instance(type_idx) => {
                                let maybe_tpe = self.get_component().get_component_type(*type_idx);
                                let tpe = AnalysisFailure::fail_on_missing(
                                    maybe_tpe,
                                    format!("type {type_idx}"),
                                )?;
                                self.follow_redirects(tpe)
                            }
                            ComponentTypeRef::Component(type_idx) => {
                                let maybe_tpe = self.get_component().get_component_type(*type_idx);
                                let tpe = AnalysisFailure::fail_on_missing(
                                    maybe_tpe,
                                    format!("type {type_idx}"),
                                )?;
                                self.follow_redirects(tpe)
                            }
                        }
                    }
                    None => Err(AnalysisFailure::failed(format!(
                        "Could not find exported element {name} in instance type declaration"
                    ))),
                }
            }
            _ => Err(AnalysisFailure::failed(format!(
                "Expected instance or imported instance, but got {} instead",
                instance_section.type_name()
            ))),
        }
    }

    fn find_export_by_name(
        &self,
        instance: &ComponentInstance,
        name: &String,
    ) -> AnalysisResult<(Option<Mrc<ComponentExport>>, AnalysisContext<Ast>)> {
        match instance {
            ComponentInstance::Instantiate { component_idx, .. } => {
                let (component, next_ctx) = self
                    .get_final_referenced(format!("component {component_idx}"), |component| {
                        component.get_component(*component_idx)
                    })?;
                let component = Mrc::map(component, |c| c.as_component());
                let export = component
                    .exports()
                    .iter()
                    .find(|export| export.name == *name)
                    .cloned();
                Ok((export, next_ctx.push_component(component, *component_idx)))
            }
            ComponentInstance::FromExports { exports } => {
                let export = exports.iter().find(|export| export.name == *name).cloned();
                Ok((export.map(Mrc::new), self.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::analysed_type::{f32, field, handle, record, result, str, u32, u64};
    use crate::analysis::{
        AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedResourceId,
        AnalysedResourceMode,
    };
    use test_r::test;

    #[test]
    fn analysed_function_kind() {
        let cons = AnalysedFunction {
            name: "[constructor]cart".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "user-id".to_string(),
                typ: str(),
            }],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
            }],
        };
        let method = AnalysedFunction {
            name: "[method]cart.add-item".to_string(),
            parameters: vec![
                AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                },
                AnalysedFunctionParameter {
                    name: "item".to_string(),
                    typ: record(vec![
                        field("product-id", str()),
                        field("name", str()),
                        field("price", f32()),
                        field("quantity", u32()),
                    ]),
                },
            ],
            results: vec![],
        };
        let static_method = AnalysedFunction {
            name: "[static]cart.merge".to_string(),
            parameters: vec![
                AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                },
                AnalysedFunctionParameter {
                    name: "that".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                },
            ],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
            }],
        };
        let fun = AnalysedFunction {
            name: "hash".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "path".to_string(),
                typ: str(),
            }],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: result(
                    record(vec![field("lower", u64()), field("upper", u64())]),
                    str(),
                ),
            }],
        };

        assert!(cons.is_constructor());
        assert!(method.is_method());
        assert!(static_method.is_static_method());
        assert!(!fun.is_constructor());
        assert!(!fun.is_method());
        assert!(!fun.is_static_method());
    }
}
