// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::bridge_gen::type_naming::analyzed_type_ext::AnalysedTypeExt;
use crate::bridge_gen::type_naming::builder::{Builder, RootOwner};
use golem_common::base_model::agent::{AgentType, DataSchema, ElementSchema};
use golem_wasm::analysis::AnalysedType;
use heck::ToPascalCase;
use indexmap::IndexMap;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use type_location::{TypeLocation, TypeLocationPath};

mod analyzed_type_ext;
mod builder;
mod type_location;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeName {
    name: String,
    owner: Option<String>,
}

impl Display for TypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(owner) = &self.owner {
            write!(f, "{}::", owner)?;
        }
        write!(f, "{}", self.name)
    }
}

pub struct TypeNaming {
    type_should_be_named: fn(&AnalysedType) -> bool,

    named_type_locations: IndexMap<TypeName, IndexMap<AnalysedType, Vec<TypeLocation>>>,
    anonymous_type_locations: IndexMap<AnalysedType, Vec<TypeLocation>>,
    type_names: HashSet<TypeName>,
    types: HashMap<AnalysedType, TypeName>,
}

impl TypeNaming {
    pub fn new(agent_type: &AgentType, type_should_be_named: fn(&AnalysedType) -> bool) -> Self {
        let mut type_naming = Self {
            type_should_be_named,
            named_type_locations: Default::default(),
            anonymous_type_locations: Default::default(),
            type_names: HashSet::new(),
            types: HashMap::new(),
        };

        type_naming.collect_all_wit_types(agent_type);
        type_naming.derive_type_names();

        type_naming
    }

    fn collect_all_wit_types(&mut self, agent_type: &AgentType) {
        let mut builder = Builder::new();

        self.collect_wit_types_in_data_schema(&mut builder, &agent_type.constructor.input_schema);
        for method in &agent_type.methods {
            builder.set_root_owner(RootOwner::MethodInput {
                method_name: method.name.clone(),
            });
            self.collect_wit_types_in_data_schema(&mut builder, &method.input_schema);
            builder.set_root_owner(RootOwner::MethodOutput {
                method_name: method.name.clone(),
            });
            self.collect_wit_types_in_data_schema(&mut builder, &method.output_schema);
        }
    }

    fn collect_wit_types_in_data_schema(&mut self, builder: &mut Builder, schema: &DataSchema) {
        match schema {
            DataSchema::Tuple(items) => {
                for named_item in &items.elements {
                    builder.set_root_item_name(&named_item.name);
                    self.collect_wit_types_in_element_schema(builder, &named_item.schema);
                }
            }
            DataSchema::Multimodal(variants) => {
                for named_variant in &variants.elements {
                    builder.set_root_item_name(&named_variant.name);
                    self.collect_wit_types_in_element_schema(builder, &named_variant.schema);
                }
            }
        }
    }

    fn collect_wit_types_in_element_schema(
        &mut self,
        builder: &mut Builder,
        schema: &ElementSchema,
    ) {
        let ElementSchema::ComponentModel(component_model_type) = schema else {
            return;
        };

        self.collect_analysed_type(builder, &component_model_type.element_type);
    }

    fn collect_analysed_type(&mut self, builder: &mut Builder, typ: &AnalysedType) {
        match typ {
            AnalysedType::Variant(variant) => {
                for case in &variant.cases {
                    if let Some(typ) = case.typ.path_elem_type() {
                        builder.push(TypeLocationPath::VariantCase {
                            name: variant.name.clone(),
                            owner: variant.owner.clone(),
                            case: case.name.clone(),
                            inner: None,
                        });
                        self.collect_analysed_type(builder, typ);
                        builder.pop();
                    }
                }
            }
            AnalysedType::Result(result) => {
                if let Some(ok) = result.ok.path_elem_type() {
                    builder.push(TypeLocationPath::ResultOk {
                        name: result.name.clone(),
                        owner: result.owner.clone(),
                        inner: None,
                    });
                    self.collect_analysed_type(builder, ok);
                    builder.pop()
                }
                if let Some(err) = result.err.path_elem_type() {
                    builder.push(TypeLocationPath::ResultErr {
                        name: result.name.clone(),
                        owner: result.owner.clone(),
                        inner: None,
                    });
                    self.collect_analysed_type(builder, err);
                    builder.pop()
                }
            }
            AnalysedType::Option(option) => {
                if let Some(inner) = option.inner.path_elem_type() {
                    builder.push(TypeLocationPath::Option {
                        name: option.name.clone(),
                        owner: option.owner.clone(),
                        inner: None,
                    });
                    self.collect_analysed_type(builder, inner);
                    builder.pop();
                }
            }
            AnalysedType::Record(record) => {
                for field in &record.fields {
                    if let Some(typ) = field.typ.path_elem_type() {
                        builder.push(TypeLocationPath::RecordField {
                            name: record.name.clone(),
                            owner: record.owner.clone(),
                            field_name: field.name.clone(),
                            inner: None,
                        });
                        self.collect_analysed_type(builder, &typ);
                        builder.pop();
                    }
                }
            }
            AnalysedType::Tuple(tuple) => {
                for (idx, item) in tuple.items.iter().enumerate() {
                    if let Some(typ) = item.path_elem_type() {
                        builder.push(TypeLocationPath::TupleItem {
                            name: tuple.name.clone(),
                            owner: tuple.owner.clone(),
                            idx: idx.to_string(),
                            inner: None,
                        });
                        self.collect_analysed_type(builder, typ);
                        builder.pop();
                    }
                }
            }
            AnalysedType::List(list) => {
                if let Some(inner) = list.inner.path_elem_type() {
                    builder.push(TypeLocationPath::List {
                        name: list.name.clone(),
                        owner: list.owner.clone(),
                        inner: None,
                    });
                    self.collect_analysed_type(builder, inner);
                    builder.pop();
                }
            }
            AnalysedType::Enum(_)
            | AnalysedType::Flags(_)
            | AnalysedType::Str(_)
            | AnalysedType::Chr(_)
            | AnalysedType::F64(_)
            | AnalysedType::F32(_)
            | AnalysedType::U64(_)
            | AnalysedType::S64(_)
            | AnalysedType::U32(_)
            | AnalysedType::S32(_)
            | AnalysedType::U16(_)
            | AnalysedType::S16(_)
            | AnalysedType::U8(_)
            | AnalysedType::S8(_)
            | AnalysedType::Bool(_)
            | AnalysedType::Handle(_) => {
                // NOP
            }
        }

        if typ.can_be_named() {
            match typ.name() {
                Some(name) => self
                    .named_type_locations
                    .entry(TypeName {
                        name: name.to_string(),
                        owner: typ.owner().map(|owner| owner.to_string()),
                    })
                    .or_default()
                    .entry(typ.clone())
                    .or_default()
                    .push(builder.type_location()),
                None => {
                    if (self.type_should_be_named)(typ) {
                        self.anonymous_type_locations
                            .entry(typ.clone())
                            .or_default()
                            .push(builder.type_location());
                    }
                }
            }
        }
    }

    fn derive_type_names(&mut self) {
        for (name, type_to_locations) in self.named_type_locations.clone() {
            let force_generate_unique_name_by_location = type_to_locations.len() > 1;
            for (typ, locations) in type_to_locations {
                self.add_unique_type(
                    Some(name.clone()),
                    typ,
                    &locations,
                    force_generate_unique_name_by_location,
                );
            }
        }
        for (typ, locations) in self.anonymous_type_locations.clone() {
            if (self.type_should_be_named)(&typ) {
                self.add_unique_type(None, typ, &locations, false);
            }
        }
    }

    fn add_unique_type(
        &mut self,
        name: Option<TypeName>,
        typ: AnalysedType,
        locations: &[TypeLocation],
        force_generate_unique_by_location: bool,
    ) {
        if self.types.contains_key(&typ) {
            return;
        }

        let name = match name {
            Some(name) => {
                if force_generate_unique_by_location || self.type_names.contains(&name) {
                    self.generate_unique_type_name_based_on_locations(Some(&name), locations)
                } else {
                    name
                }
            }
            None => self.generate_unique_type_name_based_on_locations(None, locations),
        };

        if self.type_names.contains(&name) {
            todo!("collision")
        }

        self.type_names.insert(name.clone());
        self.types.insert(typ, name);
    }

    fn generate_unique_type_name_based_on_locations(
        &self,
        name: Option<&TypeName>,
        locations: &[TypeLocation],
    ) -> TypeName {
        for location in locations {
            let segments = location.to_type_naming_segments();
            let len = segments.len();
            let mut candidate = match name {
                Some(name) => name.name.clone(),
                None => "".to_string(),
            };
            for i in (0..len).rev() {
                candidate = format!(
                    "{}{}",
                    segments[i].iter().map(|s| s.to_pascal_case()).join(""),
                    candidate
                );
                let type_name = TypeName {
                    name: candidate.clone(),
                    owner: None,
                };
                if !self.type_names.contains(&type_name) {
                    return type_name;
                }
            }
        }
        todo!("collision")
    }
}
