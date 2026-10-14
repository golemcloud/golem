// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::model::masking::MaskingConfig;
use anyhow::{anyhow, bail};
use serde::Serialize;
use serde::Serializer;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, VecDeque};

pub const CLI_OUTPUT_TYPE_FIELD: &str = "$type";
const CLI_OUTPUT_TYPES_FIELD: &str = "x-golem-cli-output-types";
pub const COMMAND_OUTPUT_SCHEMA_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/command-output-schema/command-output.schema.json"
));

pub trait StructuredOutput: Serialize {
    const KIND: &'static str;

    fn type_name() -> String {
        Self::KIND.to_string()
    }

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        Self: Sized,
    {
        let _ = config;
        self.serialize(serializer)
    }
}

pub fn command_output_schema_value() -> anyhow::Result<Value> {
    serde_json::from_str(COMMAND_OUTPUT_SCHEMA_JSON)
        .map_err(|err| anyhow!("Embedded command output schema must parse: {err}"))
}

pub fn command_output_type_names() -> anyhow::Result<Value> {
    let schema = command_output_schema_value()?;
    let entries = schema_output_type_entries(&schema)?;
    let names = entries
        .iter()
        .filter_map(|entry| entry.get("type"))
        .filter_map(Value::as_str)
        .map(|name| Value::String(name.to_string()))
        .collect::<Vec<_>>();
    Ok(Value::Array(names))
}

pub fn focused_command_output_schema(output_types: &[String]) -> anyhow::Result<Value> {
    if output_types.is_empty() {
        bail!("At least one output type must be specified");
    }

    let schema = command_output_schema_value()?;
    let definitions = schema
        .get("definitions")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Command output schema is missing definitions"))?;
    let output_type_entries = schema_output_type_entries(&schema)?;
    let known_output_types = output_type_entries
        .iter()
        .filter_map(|entry| entry.get("type"))
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();

    let mut selected = BTreeSet::<String>::new();
    let mut reachable = BTreeSet::<String>::new();
    let mut queue = VecDeque::<String>::new();
    for output_type in output_types {
        if !known_output_types.contains(output_type.as_str()) {
            bail!(
                "Unknown output type: {output_type}; run `golem output-schema --types` to list known output types"
            );
        }
        if !definitions.contains_key(output_type) {
            bail!("Command output schema is missing definition {output_type}");
        }
        if selected.insert(output_type.clone()) && reachable.insert(output_type.clone()) {
            queue.push_back(output_type.clone());
        }
    }

    while let Some(name) = queue.pop_front() {
        let definition = definitions
            .get(&name)
            .ok_or_else(|| anyhow!("Command output schema is missing definition {name}"))?;
        let mut refs = BTreeSet::new();
        collect_definition_refs(definition, &mut refs);
        for reference in refs {
            if !definitions.contains_key(&reference) {
                bail!("Command output schema references missing definition {reference}");
            }
            if reachable.insert(reference.clone()) {
                queue.push_back(reference);
            }
        }
    }

    let mut focused = Map::new();
    if let Some(value) = schema.get("$schema") {
        focused.insert("$schema".to_string(), value.clone());
    }
    if let Some(value) = schema.get("title") {
        focused.insert("title".to_string(), value.clone());
    }
    focused.insert(
        "description".to_string(),
        Value::String(
            "Focused structured output schema for selected Golem CLI output types.".to_string(),
        ),
    );
    focused.insert(
        "oneOf".to_string(),
        Value::Array(
            selected
                .iter()
                .map(|output_type| json_ref(output_type))
                .collect(),
        ),
    );

    let mut pruned_definitions = Map::new();
    for name in &reachable {
        pruned_definitions.insert(
            name.clone(),
            definitions
                .get(name)
                .ok_or_else(|| anyhow!("Command output schema is missing definition {name}"))?
                .clone(),
        );
    }
    focused.insert("definitions".to_string(), Value::Object(pruned_definitions));

    focused.insert(
        CLI_OUTPUT_TYPES_FIELD.to_string(),
        Value::Array(
            output_type_entries
                .iter()
                .filter(|entry| {
                    entry
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|output_type| selected.contains(output_type))
                })
                .cloned()
                .collect(),
        ),
    );

    Ok(Value::Object(focused))
}

fn schema_output_type_entries(schema: &Value) -> anyhow::Result<&Vec<Value>> {
    schema
        .get(CLI_OUTPUT_TYPES_FIELD)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Command output schema is missing {CLI_OUTPUT_TYPES_FIELD}"))
}

fn collect_definition_refs(value: &Value, refs: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && let Some(name) = reference.strip_prefix("#/definitions/")
            {
                refs.insert(name.to_string());
            }
            for value in object.values() {
                collect_definition_refs(value, refs);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_definition_refs(value, refs);
            }
        }
        _ => {}
    }
}

fn json_ref(definition_name: &str) -> Value {
    let mut reference = Map::new();
    reference.insert(
        "$ref".to_string(),
        Value::String(format!("#/definitions/{definition_name}")),
    );
    Value::Object(reference)
}

pub fn to_structured_output_value<Output: StructuredOutput>(
    output: Output,
) -> anyhow::Result<Value> {
    to_structured_output_value_masked(output, MaskingConfig::hide_secrets())
}

pub fn to_structured_output_value_masked<Output: StructuredOutput>(
    output: Output,
    config: MaskingConfig,
) -> anyhow::Result<Value> {
    let value = output.serialize_masked(serde_json::value::Serializer, config)?;
    let type_value = Value::String(Output::type_name());

    match value {
        Value::Object(fields) => Ok(Value::Object(with_structured_output_type::<Output>(
            fields, type_value,
        )?)),
        value => {
            let mut fields = Map::new();
            fields.insert(CLI_OUTPUT_TYPE_FIELD.to_string(), type_value);
            fields.insert("value".to_string(), value);
            Ok(Value::Object(fields))
        }
    }
}

fn with_structured_output_type<Output: StructuredOutput>(
    fields: Map<String, Value>,
    type_value: Value,
) -> anyhow::Result<Map<String, Value>> {
    let mut result = Map::new();
    result.insert(CLI_OUTPUT_TYPE_FIELD.to_string(), type_value);

    for (key, value) in fields {
        if key == CLI_OUTPUT_TYPE_FIELD {
            bail!(
                "CLI output model {} must not define reserved field {CLI_OUTPUT_TYPE_FIELD}",
                Output::KIND,
            );
        }
        result.insert(key, value);
    }

    Ok(result)
}

#[cfg(test)]
mod tests;
