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

use golem_common::model::agent::{
    ComponentModelElementValue, DataValue, ElementValue, ElementValues, NamedElementValue,
    NamedElementValues,
};
use golem_wasm::ValueAndType;
use golem_wasm::analysis::{
    AnalysedType, NameOptionTypePair, NameTypePair, TypeEnum, TypeFlags, TypeHandle, TypeList,
    TypeOption, TypeRecord, TypeResult, TypeTuple, TypeVariant,
};
use heck::{ToLowerCamelCase, ToPascalCase, ToSnakeCase};

/// Transforms an AnalysedType's naming to match Rust SDK conventions.
/// Record fields → snake_case, variant/enum cases → PascalCase, flags → snake_case,
/// type names → PascalCase.
pub fn to_rust_sdk_naming(typ: &AnalysedType) -> AnalysedType {
    transform_analysed_type(typ, &RustNaming)
}

/// Transforms an AnalysedType's naming to match TypeScript SDK conventions.
/// Record fields → camelCase, flags → camelCase, variant/enum cases unchanged,
/// type names → PascalCase.
pub fn to_ts_sdk_naming(typ: &AnalysedType) -> AnalysedType {
    transform_analysed_type(typ, &TsNaming)
}

/// Transforms a `DataValue` to use Rust SDK naming conventions.
pub fn to_rust_data_value(dv: DataValue) -> DataValue {
    transform_data_value(dv, &to_rust_sdk_naming)
}

/// Transforms a `DataValue` to use TypeScript SDK naming conventions.
pub fn to_ts_data_value(dv: DataValue) -> DataValue {
    transform_data_value(dv, &to_ts_sdk_naming)
}

/// Transforms all `ComponentModel` element values in a `DataValue` by applying
/// the given naming transformation to their types.
pub fn transform_data_value(dv: DataValue, f: &dyn Fn(&AnalysedType) -> AnalysedType) -> DataValue {
    match dv {
        DataValue::Tuple(ElementValues { elements }) => DataValue::Tuple(ElementValues {
            elements: elements
                .into_iter()
                .map(|e| transform_element_value(e, f))
                .collect(),
        }),
        DataValue::Multimodal(NamedElementValues { elements }) => {
            DataValue::Multimodal(NamedElementValues {
                elements: elements
                    .into_iter()
                    .map(|e| NamedElementValue {
                        name: e.name,
                        value: transform_element_value(e.value, f),
                        schema_index: e.schema_index,
                    })
                    .collect(),
            })
        }
    }
}

/// Transforms a single `ElementValue` by applying the given naming transformation
/// to its type (if it is a `ComponentModel` element).
pub fn transform_element_value(
    ev: ElementValue,
    f: &dyn Fn(&AnalysedType) -> AnalysedType,
) -> ElementValue {
    match ev {
        ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
            ElementValue::ComponentModel(ComponentModelElementValue {
                value: ValueAndType::new(value.value, f(&value.typ)),
            })
        }
        other => other,
    }
}

trait SdkNaming {
    fn transform_field_name(&self, name: &str) -> String;
    fn transform_case_name(&self, name: &str) -> String;
    fn transform_flag_name(&self, name: &str) -> String;
    fn transform_type_name(&self, name: &str) -> String;
}

struct RustNaming;

impl SdkNaming for RustNaming {
    fn transform_field_name(&self, name: &str) -> String {
        name.to_snake_case()
    }

    fn transform_case_name(&self, name: &str) -> String {
        name.to_pascal_case()
    }

    fn transform_flag_name(&self, name: &str) -> String {
        name.to_snake_case()
    }

    fn transform_type_name(&self, name: &str) -> String {
        name.to_pascal_case()
    }
}

struct TsNaming;

impl SdkNaming for TsNaming {
    fn transform_field_name(&self, name: &str) -> String {
        name.to_lower_camel_case()
    }

    fn transform_case_name(&self, name: &str) -> String {
        name.to_string()
    }

    fn transform_flag_name(&self, name: &str) -> String {
        name.to_lower_camel_case()
    }

    fn transform_type_name(&self, name: &str) -> String {
        name.to_pascal_case()
    }
}

fn transform_type_name(name: &Option<String>, naming: &dyn SdkNaming) -> Option<String> {
    name.as_ref().map(|n| naming.transform_type_name(n))
}

fn transform_analysed_type(typ: &AnalysedType, naming: &dyn SdkNaming) -> AnalysedType {
    match typ {
        AnalysedType::Record(record) => AnalysedType::Record(TypeRecord {
            name: transform_type_name(&record.name, naming),
            owner: record.owner.clone(),
            fields: record
                .fields
                .iter()
                .map(|f| NameTypePair {
                    name: naming.transform_field_name(&f.name),
                    typ: transform_analysed_type(&f.typ, naming),
                })
                .collect(),
        }),
        AnalysedType::Variant(variant) => AnalysedType::Variant(TypeVariant {
            name: transform_type_name(&variant.name, naming),
            owner: variant.owner.clone(),
            cases: variant
                .cases
                .iter()
                .map(|c| NameOptionTypePair {
                    name: naming.transform_case_name(&c.name),
                    typ: c.typ.as_ref().map(|t| transform_analysed_type(t, naming)),
                })
                .collect(),
        }),
        AnalysedType::Enum(enum_type) => AnalysedType::Enum(TypeEnum {
            name: transform_type_name(&enum_type.name, naming),
            owner: enum_type.owner.clone(),
            cases: enum_type
                .cases
                .iter()
                .map(|c| naming.transform_case_name(c))
                .collect(),
        }),
        AnalysedType::Flags(flags) => AnalysedType::Flags(TypeFlags {
            name: transform_type_name(&flags.name, naming),
            owner: flags.owner.clone(),
            names: flags
                .names
                .iter()
                .map(|n| naming.transform_flag_name(n))
                .collect(),
        }),
        AnalysedType::List(list) => AnalysedType::List(TypeList {
            name: transform_type_name(&list.name, naming),
            owner: list.owner.clone(),
            inner: Box::new(transform_analysed_type(&list.inner, naming)),
        }),
        AnalysedType::Option(option) => AnalysedType::Option(TypeOption {
            name: transform_type_name(&option.name, naming),
            owner: option.owner.clone(),
            inner: Box::new(transform_analysed_type(&option.inner, naming)),
        }),
        AnalysedType::Result(result) => AnalysedType::Result(TypeResult {
            name: transform_type_name(&result.name, naming),
            owner: result.owner.clone(),
            ok: result
                .ok
                .as_ref()
                .map(|t| Box::new(transform_analysed_type(t, naming))),
            err: result
                .err
                .as_ref()
                .map(|t| Box::new(transform_analysed_type(t, naming))),
        }),
        AnalysedType::Tuple(tuple) => AnalysedType::Tuple(TypeTuple {
            name: transform_type_name(&tuple.name, naming),
            owner: tuple.owner.clone(),
            items: tuple
                .items
                .iter()
                .map(|t| transform_analysed_type(t, naming))
                .collect(),
        }),
        AnalysedType::Handle(handle) => AnalysedType::Handle(TypeHandle {
            name: transform_type_name(&handle.name, naming),
            owner: handle.owner.clone(),
            resource_id: handle.resource_id,
            mode: handle.mode.clone(),
        }),
        AnalysedType::Str(_)
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
        | AnalysedType::Bool(_) => typ.clone(),
    }
}
