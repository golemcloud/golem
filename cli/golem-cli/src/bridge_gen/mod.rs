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

pub mod rust;
pub mod typescript;

use camino::Utf8Path;
use golem_common::model::agent::{AgentType, DataSchema, ElementSchema};
use golem_wasm::analysis::AnalysedType;
use std::collections::{HashSet, VecDeque};

#[allow(dead_code)]
pub trait BridgeGenerator {
    fn new(agent_type: AgentType, target_path: &Utf8Path, testing: bool) -> Self;
    fn generate(&mut self) -> anyhow::Result<()>;
}

fn collect_all_wit_types(agent_type: &AgentType) -> Vec<AnalysedType> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();

    let mut add_type = |typ: AnalysedType| {
        if seen.insert(typ.clone()) {
            result.push(typ);
        }
    };

    for typ in wit_types_in_data_schema(&agent_type.constructor.input_schema) {
        add_type(typ);
    }
    for method in &agent_type.methods {
        for typ in wit_types_in_data_schema(&method.input_schema) {
            add_type(typ);
        }
        for typ in wit_types_in_data_schema(&method.output_schema) {
            add_type(typ);
        }
    }
    result
}

fn wit_types_in_data_schema(schema: &DataSchema) -> Vec<AnalysedType> {
    let mut result = Vec::new();
    match schema {
        DataSchema::Tuple(items) => {
            for named_item in &items.elements {
                result.extend(wit_types_in_element_schema(&named_item.schema));
            }
        }
        DataSchema::Multimodal(variants) => {
            for named_variant in &variants.elements {
                result.extend(wit_types_in_element_schema(&named_variant.schema));
            }
        }
    }
    result
}

fn wit_types_in_element_schema(schema: &ElementSchema) -> Vec<AnalysedType> {
    let mut result = Vec::new();
    if let ElementSchema::ComponentModel(component_model_type) = schema {
        result.push(component_model_type.element_type.clone());
        result.extend(named_types_in_analysed_type(
            &component_model_type.element_type,
        ));
    }
    result
}

fn named_types_in_analysed_type(typ: &AnalysedType) -> Vec<AnalysedType> {
    let mut result = Vec::new();

    let mut stack = VecDeque::new();
    stack.push_back(typ);
    let mut visited = HashSet::new();

    while let Some(current) = stack.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current);

        if current.name().is_some() {
            result.push(current.clone());
        }

        match current {
            AnalysedType::Variant(variant) => {
                for case in &variant.cases {
                    if let Some(typ) = &case.typ {
                        stack.push_back(typ);
                    }
                }
            }
            AnalysedType::Result(result) => {
                if let Some(ok) = &result.ok {
                    stack.push_back(ok);
                }
                if let Some(err) = &result.err {
                    stack.push_back(err);
                }
            }
            AnalysedType::Option(inner) => {
                stack.push_back(&*inner.inner);
            }
            AnalysedType::Record(fields) => {
                for item in &fields.fields {
                    stack.push_back(&item.typ);
                }
            }
            AnalysedType::Tuple(items) => {
                for item in &items.items {
                    stack.push_back(item);
                }
            }
            AnalysedType::List(inner) => {
                stack.push_back(&*inner.inner);
            }
            _ => {}
        }
    }
    result
}
