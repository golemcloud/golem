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
use anyhow::bail;
use golem_common::base_model::agent::{AgentType, DataSchema, ElementSchema};
use golem_wasm::analysis::AnalysedType;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use type_location::{TypeLocation, TypeLocationPath};

pub(crate) mod analyzed_type_ext;
mod builder;
mod type_location;

#[cfg(test)]
pub(crate) mod tests;

pub trait TypeName: Debug + Display + Clone + PartialEq + Eq + Hash {
    // This is intended to be used for custom or special mappings. If this method returns some
    // result for a type, then no further type naming will be attempted.
    fn from_analysed_type(typ: &AnalysedType) -> Option<Self>;

    fn from_owner_and_name(owner: Option<impl AsRef<str>>, name: impl AsRef<str>) -> Self;

    fn from_segments(segments: impl IntoIterator<Item = impl AsRef<str>>) -> Self;

    fn requires_type_name(typ: &AnalysedType) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TsTypeName {
    pub name: String,
    pub owner: Option<String>,
}

impl Display for TsTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(owner) = &self.owner {
            write!(f, "{}::", owner)?;
        }
        write!(f, "{}", self.name)
    }
}

pub struct TypeNaming<TN: TypeName> {
    named_type_locations: IndexMap<TN, IndexMap<AnalysedType, Vec<TypeLocation>>>,
    anonymous_type_locations: IndexMap<AnalysedType, Vec<TypeLocation>>,
    type_names: HashSet<TN>,
    types: HashMap<AnalysedType, TN>,
}

impl<TN: TypeName> TypeNaming<TN> {
    pub fn new(agent_type: &AgentType) -> anyhow::Result<Self> {
        let mut type_naming = Self {
            named_type_locations: Default::default(),
            anonymous_type_locations: Default::default(),
            type_names: HashSet::new(),
            types: HashMap::new(),
        };

        type_naming.collect_all_wit_types(agent_type);
        type_naming.derive_type_names()?;

        Ok(type_naming)
    }

    pub fn type_name_for_type(&self, typ: &AnalysedType) -> Option<&TN> {
        self.types.get(typ)
    }

    pub fn types(&self) -> impl Iterator<Item = (&AnalysedType, &TN)> {
        self.types.iter()
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
                    if let Some(typ) = case.typ.as_path_elem_type() {
                        builder.push(TypeLocationPath::VariantCase {
                            name: variant.name.clone(),
                            owner: variant.owner.clone(),
                            case: case.name.clone(),
                            inner: None,
                        });
                        self.collect_analysed_type(builder, typ);
                        builder.pop();
                    } else if let Some(typ) = &case.typ {
                        self.collect_analysed_type(builder, typ);
                    }
                }
            }
            AnalysedType::Result(result) => {
                if let Some(ok) = result.ok.as_path_elem_type() {
                    builder.push(TypeLocationPath::ResultOk {
                        name: result.name.clone(),
                        owner: result.owner.clone(),
                        inner: None,
                    });
                    self.collect_analysed_type(builder, ok);
                    builder.pop()
                } else if let Some(ok) = &result.ok {
                    self.collect_analysed_type(builder, ok);
                }

                if let Some(err) = result.err.as_path_elem_type() {
                    builder.push(TypeLocationPath::ResultErr {
                        name: result.name.clone(),
                        owner: result.owner.clone(),
                        inner: None,
                    });
                    self.collect_analysed_type(builder, err);
                    builder.pop()
                } else if let Some(err) = &result.err {
                    self.collect_analysed_type(builder, err);
                }
            }
            AnalysedType::Option(option) => {
                if let Some(inner) = option.inner.as_path_elem_type() {
                    builder.push(TypeLocationPath::Option {
                        name: option.name.clone(),
                        owner: option.owner.clone(),
                        inner: None,
                    });
                    self.collect_analysed_type(builder, inner);
                    builder.pop();
                } else {
                    self.collect_analysed_type(builder, &option.inner);
                }
            }
            AnalysedType::Record(record) => {
                for field in &record.fields {
                    if let Some(typ) = field.typ.as_path_elem_type() {
                        builder.push(TypeLocationPath::RecordField {
                            name: record.name.clone(),
                            owner: record.owner.clone(),
                            field_name: field.name.clone(),
                            inner: None,
                        });
                        self.collect_analysed_type(builder, typ);
                        builder.pop();
                    } else {
                        self.collect_analysed_type(builder, &field.typ);
                    }
                }
            }
            AnalysedType::Tuple(tuple) => {
                for (idx, item) in tuple.items.iter().enumerate() {
                    if let Some(typ) = item.as_path_elem_type() {
                        builder.push(TypeLocationPath::TupleItem {
                            name: tuple.name.clone(),
                            owner: tuple.owner.clone(),
                            idx: idx.to_string(),
                            inner: None,
                        });
                        self.collect_analysed_type(builder, typ);
                        builder.pop();
                    } else {
                        self.collect_analysed_type(builder, item);
                    }
                }
            }
            AnalysedType::List(list) => {
                if let Some(inner) = list.inner.as_path_elem_type() {
                    builder.push(TypeLocationPath::List {
                        name: list.name.clone(),
                        owner: list.owner.clone(),
                        inner: None,
                    });
                    self.collect_analysed_type(builder, inner);
                    builder.pop();
                } else {
                    self.collect_analysed_type(builder, &list.inner);
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
                    .entry(TN::from_owner_and_name(typ.owner(), name))
                    .or_default()
                    .entry(typ.clone())
                    .or_default()
                    .push(builder.type_location()),
                None => {
                    if TN::requires_type_name(typ) {
                        self.anonymous_type_locations
                            .entry(typ.clone())
                            .or_default()
                            .push(builder.type_location());
                    }
                }
            }
        }
    }

    fn derive_type_names(&mut self) -> anyhow::Result<()> {
        for (name, type_to_locations) in self.named_type_locations.clone() {
            let force_generate_unique_name_by_location = type_to_locations.len() > 1;
            for (typ, locations) in type_to_locations {
                self.add_unique_type(
                    Some(name.clone()),
                    typ,
                    &locations,
                    force_generate_unique_name_by_location,
                )?;
            }
        }
        for (typ, locations) in self.anonymous_type_locations.clone() {
            self.add_unique_type(None, typ, &locations, false)?;
        }
        Ok(())
    }

    fn add_unique_type(
        &mut self,
        name: Option<TN>,
        typ: AnalysedType,
        locations: &[TypeLocation],
        force_generate_unique_by_location: bool,
    ) -> anyhow::Result<()> {
        if self.types.contains_key(&typ) {
            return Ok(());
        }

        let name = match name {
            Some(name) => {
                if force_generate_unique_by_location || self.type_names.contains(&name) {
                    self.generate_unique_type_name_based_on_locations(Some(&name), locations)?
                } else {
                    name
                }
            }
            None => self.generate_unique_type_name_based_on_locations(None, locations)?,
        };

        self.type_names.insert(name.clone());
        self.types.insert(typ, name);

        Ok(())
    }

    fn generate_unique_type_name_based_on_locations(
        &self,
        name: Option<&TN>,
        locations: &[TypeLocation],
    ) -> anyhow::Result<TN> {
        for location in locations {
            let segments = location.to_type_naming_segments();
            let len = segments.len();
            let mut candidate = match name {
                Some(name) => name.to_string(),
                None => "".to_string(),
            };
            for i in (0..len).rev() {
                let subsegments = &segments[i];
                if subsegments.is_empty() {
                    continue;
                }
                let candidate_type_name = TN::from_segments(
                    subsegments
                        .iter()
                        .copied()
                        .chain(std::iter::once(candidate.as_str())),
                );
                if !self.type_names.contains(&candidate_type_name) {
                    return Ok(candidate_type_name);
                }
                candidate = candidate_type_name.to_string();
            }
        }
        bail!(
            "Failed to generate unique location based type name for {:#?}\n\nlocations: {:#?}",
            name,
            locations,
        )
    }
}
