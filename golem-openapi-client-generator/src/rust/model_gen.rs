// Copyright 2024 Golem Cloud
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

use crate::printer::{indented, NewLine};
use crate::rust::lib_gen::{Module, ModuleDef, ModuleName};
use crate::rust::printer::{line, rust_name, unit, RustContext};
use crate::rust::types::{
    ref_or_box_schema_type, ref_type_name, DataType, RustPrinter, RustResult,
};
use crate::Error;
use crate::Result;
use convert_case::{Case, Casing};
use openapiv3::{
    AnySchema, Discriminator, OpenAPI, ReferenceOr, Schema, SchemaData, SchemaKind, Type,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct RefCache {
    pub refs: HashSet<String>,
}

impl RefCache {
    pub fn new() -> RefCache {
        RefCache {
            refs: HashSet::new(),
        }
    }

    pub fn add<S: Into<String>>(&mut self, s: S) {
        self.refs.insert(s.into());
    }

    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }
}

fn serialize() -> RustPrinter {
    rust_name("serde", "Serialize")
}

fn deserialize() -> RustPrinter {
    rust_name("serde", "Deserialize")
}

fn derive_line() -> RustPrinter {
    line(unit() + "#[derive(Debug, Clone, PartialEq, " + serialize() + ", " + deserialize() + ")]")
}

fn derive_line_simple() -> RustPrinter {
    line(unit() + "#[derive(Debug, Clone, PartialEq, " + serialize() + ", " + deserialize() + ")]")
}

fn rename_line(to: &str) -> RustPrinter {
    line(unit() + r#"#[serde(rename = ""# + to + r#"")]"#)
}

fn enum_schema_sanity_check(any_schema: &AnySchema, data: &SchemaData) -> Result<()> {
    if !any_schema.any_of.is_empty() {
        Err(Error::unimplemented(
            "oneOf expected for enum, but got anyOf",
        ))
    } else if !any_schema.all_of.is_empty() {
        Err(Error::unimplemented(
            "oneOf expected for enum, but got allOf",
        ))
    } else if any_schema.one_of.is_empty() {
        Err(Error::unimplemented("oneOf expected for enum"))
    } else if let Some(discriminator) = &data.discriminator {
        if discriminator.mapping.len() != any_schema.one_of.len() {
            Err(Error::unimplemented(
                "Same size for one_of and discriminator mapping expected",
            ))
        } else {
            Ok(())
        }
    } else {
        Err(Error::unimplemented("discriminator expected for enum"))
    }
}

struct EnumCase {
    name: String,
    reference: String,
    data_type: DataType,
}

impl EnumCase {
    fn render(&self, parent_reference: &str, open_api: &OpenAPI) -> RustPrinter {
        let rust_name = self.name.to_case(Case::UpperCamel);

        let rename = if rust_name == self.name {
            unit()
        } else {
            rename_line(&self.name)
        };

        let data_type = self.data_type.render_declaration(false);

        let data_type = if detect_circle(open_api, parent_reference, &self.reference) {
            unit() + "Box<" + data_type + ">"
        } else {
            data_type
        };

        rename + line(unit() + rust_name + "(" + data_type + "),")
    }
}

fn detect_circle(open_api: &OpenAPI, parent_reference: &str, reference: &str) -> bool {
    fn in_schema(
        open_api: &OpenAPI,
        parent_reference: &str,
        schema: &Schema,
        visited_refs: &mut HashSet<String>,
    ) -> bool {
        match &schema.schema_kind {
            SchemaKind::Type(Type::Object(obj)) => obj
                .properties
                .iter()
                .any(|(_, s)| in_ref_or_box_schema(open_api, parent_reference, s, visited_refs)),
            SchemaKind::Any(any) => any
                .one_of
                .iter()
                .any(|s| in_ref_or_schema(open_api, parent_reference, s, visited_refs)),
            _ => false,
        }
    }

    fn in_ref_or_schema(
        open_api: &OpenAPI,
        parent_reference: &str,
        schema: &ReferenceOr<Schema>,
        visited_refs: &mut HashSet<String>,
    ) -> bool {
        match schema {
            ReferenceOr::Reference { reference } => {
                if reference == parent_reference {
                    true
                } else {
                    inner(open_api, parent_reference, reference, visited_refs)
                }
            }
            ReferenceOr::Item(schema) => {
                in_schema(open_api, parent_reference, schema, visited_refs)
            }
        }
    }

    fn in_ref_or_box_schema(
        open_api: &OpenAPI,
        parent_reference: &str,
        schema: &ReferenceOr<Box<Schema>>,
        visited_refs: &mut HashSet<String>,
    ) -> bool {
        match schema {
            ReferenceOr::Reference { reference } => {
                if reference == parent_reference {
                    true
                } else {
                    inner(open_api, parent_reference, reference, visited_refs)
                }
            }
            ReferenceOr::Item(schema) => {
                in_schema(open_api, parent_reference, schema, visited_refs)
            }
        }
    }

    fn inner(
        open_api: &OpenAPI,
        parent_reference: &str,
        reference: &str,
        visited_refs: &mut HashSet<String>,
    ) -> bool {
        if visited_refs.contains(reference) {
            // loop detected, aboard
            false
        } else {
            visited_refs.insert(reference.to_string());

            let res = if let Some(schema_name) = reference.strip_prefix("#/components/schemas/") {
                if let Some(schema) = open_api
                    .components
                    .as_ref()
                    .unwrap()
                    .schemas
                    .get(schema_name)
                {
                    in_ref_or_schema(open_api, parent_reference, schema, visited_refs)
                } else {
                    false
                }
            } else {
                false
            };

            visited_refs.remove(reference);

            res
        }
    }

    inner(open_api, parent_reference, reference, &mut HashSet::new())
}

fn enum_case_sanity_check(property_name: &str, all_of: &[ReferenceOr<Schema>]) -> Result<()> {
    if all_of.len() != 2 {
        Err(Error::unimplemented(
            "Exactly 2 elements expected for allOf in enum case schema.",
        ))
    } else if let Some(discriminator_schema) = all_of.iter().find_map(|s| s.as_item()) {
        match &discriminator_schema.schema_kind {
            SchemaKind::Type(Type::Object(obj)) => {
                if obj.properties.len() != 1 {
                    Err(Error::unimplemented("Exactly 1 property expected for discriminator property in enum case schema."))
                } else {
                    let (name, schema) = obj.properties.first().unwrap();

                    if name != property_name {
                        Err(Error::unimplemented("Discriminator property name in case schema should be the same as in discriminator."))
                    } else if let Some(schema) = schema.as_item() {
                        match schema.schema_kind {
                            SchemaKind::Type(Type::String(_)) => Ok(()),
                            _ => Err(Error::unimplemented(
                                "String type expected for discriminator property.",
                            )),
                        }
                    } else {
                        Err(Error::unimplemented(
                            "Reference in discriminator property schema.",
                        ))
                    }
                }
            }
            _ => Err(Error::unimplemented(
                "Type 'object' schema expected for discriminator property in enum case schema.",
            )),
        }
    } else {
        Err(Error::unimplemented(
            "No discriminator property schema in enum case schema.",
        ))
    }
}

fn as_ref<T>(ref_or: &ReferenceOr<T>) -> Option<&str> {
    match ref_or {
        ReferenceOr::Reference { reference } => Some(reference),
        ReferenceOr::Item(_) => None,
    }
}

fn extract_enum_case(
    open_api: &OpenAPI,
    name: &str,
    reference: &str,
    property_name: &str,
    ref_cache: &mut RefCache,
) -> Result<EnumCase> {
    let schema_name =
        reference
            .strip_prefix("#/components/schemas/")
            .ok_or(Error::unimplemented(format!(
                "Unexpected case reference format: {reference}."
            )))?;

    let schema = open_api
        .components
        .as_ref()
        .unwrap()
        .schemas
        .get(schema_name)
        .ok_or(Error::unexpected(format!(
            "Can't find case schema by reference {schema_name}"
        )))?;

    let schema = schema.as_item().ok_or(Error::unimplemented(format!(
        "Direct cross reference in enum case {reference}"
    )))?;

    match &schema.schema_kind {
        SchemaKind::AllOf { all_of } => {
            enum_case_sanity_check(property_name, all_of)
                .map_err(|e| e.extend(format!("In enum case $ref {reference}.")))?;

            if let Some(reference) = all_of.iter().find_map(|s| as_ref(s)) {
                Ok(EnumCase {
                    name: name.to_string(),
                    reference: reference.to_string(),
                    data_type: ref_type_name(reference, ref_cache)?,
                })
            } else {
                Err(Error::unimplemented(format!(
                    "Can't find model type reference in enum case schema {schema_name}.",
                )))
            }
        }
        _ => Err(Error::unimplemented(format!(
            "allOf schema expected for enum case in {schema_name}"
        ))),
    }
}

fn extract_enum_cases(
    open_api: &OpenAPI,
    discriminator: &Discriminator,
    ref_cache: &mut RefCache,
) -> Result<Vec<EnumCase>> {
    discriminator
        .mapping
        .iter()
        .map(|(name, reference)| {
            extract_enum_case(
                open_api,
                name,
                reference,
                &discriminator.property_name,
                ref_cache,
            )
        })
        .collect()
}

pub fn multipart_field_module() -> Result<Module> {
    let code = unit()
        + line(unit() + "pub trait MultipartField {")
        + indented(
            unit()
                + line("fn to_multipart_field(&self) -> String;")
                + line("fn mime_type(&self) -> &'static str;"),
        )
        + line(unit() + "}");

    Ok(Module {
        def: ModuleDef {
            name: ModuleName::new("multipart_field"),
            exports: vec!["MultipartField".to_string()],
        },
        code: RustContext::new().print_to_string(code),
    })
}

pub fn model_gen(
    reference: &str,
    open_api: &OpenAPI,
    mapping: &HashMap<&str, &str>,
    ref_cache: &mut RefCache,
) -> Result<Module> {
    let schemas = &open_api
        .components
        .as_ref()
        .ok_or(Error::unexpected("No components."))?
        .schemas;

    let original_name =
        reference
            .strip_prefix("#/components/schemas/")
            .ok_or(Error::unimplemented(format!(
                "Unexpected reference format: {reference}."
            )))?;

    let mod_name = ModuleName::new(original_name);
    let name = mod_name.name().to_case(Case::UpperCamel);

    let schema = schemas.get(original_name).ok_or(Error::unexpected(format!(
        "Can't find schema by reference {original_name}"
    )))?;

    let schema = schema.as_item().ok_or(Error::unimplemented(format!(
        "Direct cross reference in {reference}"
    )))?;

    let code = if let Some(mapped_type) = mapping.get(name.as_str()) {
        Ok(unit() + line(unit() + "pub type " + &name + " = " + *mapped_type + ";"))
    } else {
        match &schema.schema_kind {
            SchemaKind::Type(tpe) => match tpe {
                Type::String(string_type) => {
                    if string_type.enumeration.is_empty() {
                        Err(Error::unimplemented(format!(
                            "String schema without enum {reference}"
                        )))
                    } else if string_type.enumeration.contains(&None) {
                        Err(Error::unimplemented(format!(
                            "String schema enum with empty string {reference}"
                        )))
                    } else {
                        fn make_case(name: &str) -> RustPrinter {
                            let rust_name = name.to_case(Case::UpperCamel);

                            let rename = if name == rust_name {
                                unit()
                            } else {
                                rename_line(name)
                            };

                            rename + line(unit() + rust_name + ",")
                        }

                        let cases = string_type
                            .enumeration
                            .iter()
                            .map(|n| make_case(n.as_ref().unwrap()))
                            .reduce(|acc, e| acc + e)
                            .unwrap_or_else(unit);

                        #[rustfmt::skip]
                        fn make_match_case(enum_name: &str, name: &str) -> RustPrinter {
                            let rust_name = name.to_case(Case::UpperCamel);

                            line(unit() + enum_name + "::" + rust_name + r#" => write!(f, ""# + name + r#""),"#)
                        }

                        let match_cases = string_type
                            .enumeration
                            .iter()
                            .map(|n| make_match_case(&name, n.as_ref().unwrap()))
                            .reduce(|acc, e| acc + e)
                            .unwrap_or_else(unit);

                        #[rustfmt::skip]
                        let code = unit() +
                            derive_line() +
                            line(unit() + "pub enum " + &name + " {") +
                            indented(
                                cases
                            ) +
                            line(unit() + "}") +
                            NewLine +
                            line(unit() + "impl " + rust_name("std::fmt", "Display") + " for " + &name + "{") +
                            indented(
                                line(unit() + "fn fmt(&self, f: &mut " + rust_name("std::fmt", "Formatter") + "<'_>) -> " + rust_name("std::fmt", "Result") + " {") +
                                indented(
                                    line("match self {") +
                                    indented(
                                        match_cases
                                    ) +
                                    line("}")
                                ) +
                                line("}")
                            ) +
                            line("}");

                        Ok(code)
                    }
                }
                Type::Number(_) => Err(Error::unimplemented(format!("Number schema {reference}"))),
                Type::Integer(_) => {
                    Err(Error::unimplemented(format!("Integer schema {reference}")))
                }
                Type::Boolean(_) => {
                    Err(Error::unimplemented(format!("Boolean schema {reference}")))
                }
                Type::Array(_) => Err(Error::unimplemented(format!("Array schema {reference}"))),
                Type::Object(obj) => {
                    let required: HashSet<String> =
                        obj.required.iter().map(|s| s.to_owned()).collect();

                    fn make_field(
                        name: &str,
                        schema: &ReferenceOr<Box<Schema>>,
                        required: &HashSet<String>,
                        ref_cache: &mut RefCache,
                    ) -> RustResult {
                        let rust_name = name.to_case(Case::Snake);

                        let rename = if rust_name == name {
                            unit()
                        } else {
                            rename_line(name)
                        };

                        let tpe =
                            ref_or_box_schema_type(schema, ref_cache)?.render_declaration(false);

                        let tpe = if required.contains(name) {
                            tpe
                        } else {
                            unit() + "Option<" + tpe + ">"
                        };

                        Ok(rename + line(unit() + "pub " + rust_name + ": " + tpe + ","))
                    }

                    let fields: Result<Vec<RustPrinter>> = obj
                        .properties
                        .iter()
                        .map(|(name, schema)| make_field(name, schema, &required, ref_cache))
                        .collect();

                    let fields =
                        fields.map_err(|e| e.extend(format!("In reference {reference}.")))?;

                    let code = unit()
                        + derive_line()
                        + line(unit() + "pub struct " + &name + " {")
                        + indented(
                            fields
                                .into_iter()
                                .reduce(|acc, e| acc + e)
                                .unwrap_or_else(unit),
                        )
                        + line(unit() + "}")
                        + NewLine
                        + line(
                            unit()
                                + "impl "
                                + rust_name("crate::model", "MultipartField")
                                + " for "
                                + &name
                                + "{",
                        )
                        + indented(
                            line(unit() + "fn to_multipart_field(&self) -> String {")
                                + indented(line("serde_json::to_string(self).unwrap()"))
                                + line("}")
                                + NewLine
                                + line(unit() + "fn mime_type(&self) -> &'static str {")
                                + indented(line(r#""application/json""#))
                                + line("}"),
                        )
                        + line("}");

                    Ok(code)
                }
            },
            SchemaKind::OneOf { .. } => {
                Err(Error::unimplemented(format!("OneOf schema {reference}")))
            }
            SchemaKind::AllOf { .. } => {
                Err(Error::unimplemented(format!("AllOf schema {reference}")))
            }
            SchemaKind::AnyOf { .. } => {
                Err(Error::unimplemented(format!("AnyOf schema {reference}")))
            }
            SchemaKind::Not { .. } => Err(Error::unimplemented(format!("Not schema {reference}"))),
            SchemaKind::Any(any) => {
                enum_schema_sanity_check(any, &schema.schema_data)?;

                let discriminator = schema.schema_data.discriminator.as_ref().unwrap();

                let cases = extract_enum_cases(open_api, discriminator, ref_cache);

                let cases = cases?
                    .iter()
                    .map(|c| c.render(reference, open_api))
                    .reduce(|acc, e| acc + e)
                    .unwrap_or_else(unit);

                let code = unit()
                    + derive_line_simple()
                    + line(unit() + r#"#[serde(tag = ""# + &discriminator.property_name + r#"")]"#)
                    + line(unit() + "pub enum " + &name + " {")
                    + indented(cases)
                    + line(unit() + "}");

                Ok(code)
            }
        }
    };

    let name = ModuleName::new(name);
    Ok(Module {
        def: ModuleDef {
            name: name.clone(),
            exports: vec![name.name().to_case(Case::Pascal)],
        },
        code: RustContext::new().print_to_string(code?),
    })
}
