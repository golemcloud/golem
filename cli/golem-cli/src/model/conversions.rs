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

use crate::model::GolemError;
use golem_client::model::{
    AnalysedFunction, AnalysedResourceMode, AnalysedType, NameOptionTypePair, NameTypePair,
    TypeAnnotatedValue, TypeBool, TypeChr, TypeEnum, TypeF32, TypeF64, TypeFlags, TypeHandle,
    TypeList, TypeOption, TypeRecord, TypeResult, TypeS16, TypeS32, TypeS64, TypeS8, TypeStr,
    TypeTuple, TypeU16, TypeU32, TypeU64, TypeU8, TypeVariant,
};
use golem_wasm_ast::analysis;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
// These are conversions between the OpenAPI-generated 'client types' and their corresponding 'model types'.
// It should be removed in the future when the client generator can be configured to directly map to the model types.

pub fn analysed_function_client_to_model(func: &AnalysedFunction) -> analysis::AnalysedFunction {
    analysis::AnalysedFunction {
        name: func.name.clone(),
        parameters: func
            .parameters
            .iter()
            .map(|p| analysis::AnalysedFunctionParameter {
                name: p.name.to_string(),
                typ: analysed_type_client_to_model(&p.typ),
            })
            .collect(),
        results: func
            .results
            .iter()
            .map(|r| analysis::AnalysedFunctionResult {
                name: r.name.clone(),
                typ: analysed_type_client_to_model(&r.typ),
            })
            .collect(),
    }
}

pub fn analysed_type_client_to_model(typ: &AnalysedType) -> analysis::AnalysedType {
    fn variant_to_analysed(tv: &TypeVariant) -> analysis::AnalysedType {
        analysis::AnalysedType::Variant(analysis::TypeVariant {
            cases: tv
                .cases
                .iter()
                .map(|notp| analysis::NameOptionTypePair {
                    name: notp.name.clone(),
                    typ: notp.typ.as_ref().map(analysed_type_client_to_model),
                })
                .collect(),
        })
    }

    fn result_to_analysed(tr: &TypeResult) -> analysis::AnalysedType {
        analysis::AnalysedType::Result(analysis::TypeResult {
            ok: tr
                .ok
                .as_ref()
                .map(|t| Box::new(analysed_type_client_to_model(t))),
            err: tr
                .err
                .as_ref()
                .map(|t| Box::new(analysed_type_client_to_model(t))),
        })
    }

    fn record_to_analysed(tr: &TypeRecord) -> analysis::AnalysedType {
        analysis::AnalysedType::Record(analysis::TypeRecord {
            fields: tr
                .fields
                .iter()
                .map(|ntp| analysis::NameTypePair {
                    name: ntp.name.clone(),
                    typ: analysed_type_client_to_model(&ntp.typ),
                })
                .collect(),
        })
    }

    fn handle_to_analysed(th: &TypeHandle) -> analysis::AnalysedType {
        analysis::AnalysedType::Handle(analysis::TypeHandle {
            resource_id: analysis::AnalysedResourceId(th.resource_id),
            mode: match th.mode {
                AnalysedResourceMode::Borrowed => analysis::AnalysedResourceMode::Borrowed,
                AnalysedResourceMode::Owned => analysis::AnalysedResourceMode::Owned,
            },
        })
    }

    match typ {
        AnalysedType::Variant(tv) => variant_to_analysed(tv),
        AnalysedType::Result(tr) => result_to_analysed(tr),
        AnalysedType::Option(to) => analysis::AnalysedType::Option(analysis::TypeOption {
            inner: Box::new(analysed_type_client_to_model(&to.inner)),
        }),
        AnalysedType::Enum(te) => analysis::AnalysedType::Enum(analysis::TypeEnum {
            cases: te.cases.clone(),
        }),
        AnalysedType::Flags(tf) => analysis::AnalysedType::Flags(analysis::TypeFlags {
            names: tf.names.clone(),
        }),
        AnalysedType::Record(tr) => record_to_analysed(tr),
        AnalysedType::Tuple(tt) => analysis::AnalysedType::Tuple(analysis::TypeTuple {
            items: tt.items.iter().map(analysed_type_client_to_model).collect(),
        }),
        AnalysedType::List(tl) => analysis::AnalysedType::List(analysis::TypeList {
            inner: Box::new(analysed_type_client_to_model(&tl.inner)),
        }),
        AnalysedType::Str(_) => analysis::AnalysedType::Str(analysis::TypeStr),
        AnalysedType::Chr(_) => analysis::AnalysedType::Chr(analysis::TypeChr),
        AnalysedType::F64(_) => analysis::AnalysedType::F64(analysis::TypeF64),
        AnalysedType::F32(_) => analysis::AnalysedType::F32(analysis::TypeF32),
        AnalysedType::U64(_) => analysis::AnalysedType::U64(analysis::TypeU64),
        AnalysedType::S64(_) => analysis::AnalysedType::S64(analysis::TypeS64),
        AnalysedType::U32(_) => analysis::AnalysedType::U32(analysis::TypeU32),
        AnalysedType::S32(_) => analysis::AnalysedType::S32(analysis::TypeS32),
        AnalysedType::U16(_) => analysis::AnalysedType::U16(analysis::TypeU16),
        AnalysedType::S16(_) => analysis::AnalysedType::S16(analysis::TypeS16),
        AnalysedType::U8(_) => analysis::AnalysedType::U8(analysis::TypeU8),
        AnalysedType::S8(_) => analysis::AnalysedType::S8(analysis::TypeS8),
        AnalysedType::Bool(_) => analysis::AnalysedType::Bool(analysis::TypeBool),
        AnalysedType::Handle(th) => handle_to_analysed(th),
    }
}

pub fn analysed_type_model_to_client(typ: &analysis::AnalysedType) -> AnalysedType {
    match typ {
        analysis::AnalysedType::Bool(_) => AnalysedType::Bool(TypeBool {}),
        analysis::AnalysedType::S8(_) => AnalysedType::S8(TypeS8 {}),
        analysis::AnalysedType::U8(_) => AnalysedType::U8(TypeU8 {}),
        analysis::AnalysedType::S16(_) => AnalysedType::S16(TypeS16 {}),
        analysis::AnalysedType::U16(_) => AnalysedType::U16(TypeU16 {}),
        analysis::AnalysedType::S32(_) => AnalysedType::S32(TypeS32 {}),
        analysis::AnalysedType::U32(_) => AnalysedType::U32(TypeU32 {}),
        analysis::AnalysedType::S64(_) => AnalysedType::S64(TypeS64 {}),
        analysis::AnalysedType::U64(_) => AnalysedType::U64(TypeU64 {}),
        analysis::AnalysedType::F32(_) => AnalysedType::F32(TypeF32 {}),
        analysis::AnalysedType::F64(_) => AnalysedType::F64(TypeF64 {}),
        analysis::AnalysedType::Chr(_) => AnalysedType::Chr(TypeChr {}),
        analysis::AnalysedType::Str(_) => AnalysedType::Str(TypeStr {}),
        analysis::AnalysedType::List(tl) => AnalysedType::List(Box::new(TypeList {
            inner: analysed_type_model_to_client(&tl.inner),
        })),
        analysis::AnalysedType::Tuple(tt) => AnalysedType::Tuple(TypeTuple {
            items: tt.items.iter().map(analysed_type_model_to_client).collect(),
        }),
        analysis::AnalysedType::Record(tr) => AnalysedType::Record(TypeRecord {
            fields: tr
                .fields
                .iter()
                .map(|ntp| NameTypePair {
                    name: ntp.name.clone(),
                    typ: analysed_type_model_to_client(&ntp.typ),
                })
                .collect(),
        }),
        analysis::AnalysedType::Flags(tf) => AnalysedType::Flags(TypeFlags {
            names: tf.names.clone(),
        }),
        analysis::AnalysedType::Enum(te) => AnalysedType::Enum(TypeEnum {
            cases: te.cases.clone(),
        }),
        analysis::AnalysedType::Option(to) => AnalysedType::Option(Box::new(TypeOption {
            inner: analysed_type_model_to_client(&to.inner),
        })),
        analysis::AnalysedType::Result(tr) => AnalysedType::Result(Box::new(TypeResult {
            ok: tr.ok.as_ref().map(|t| analysed_type_model_to_client(t)),
            err: tr.err.as_ref().map(|t| analysed_type_model_to_client(t)),
        })),
        analysis::AnalysedType::Variant(tv) => AnalysedType::Variant(TypeVariant {
            cases: tv
                .cases
                .iter()
                .map(|notp| NameOptionTypePair {
                    name: notp.name.clone(),
                    typ: notp.typ.as_ref().map(analysed_type_model_to_client),
                })
                .collect(),
        }),
        analysis::AnalysedType::Handle(th) => AnalysedType::Handle(TypeHandle {
            resource_id: th.resource_id.0,
            mode: match th.mode {
                analysis::AnalysedResourceMode::Borrowed => AnalysedResourceMode::Borrowed,
                analysis::AnalysedResourceMode::Owned => AnalysedResourceMode::Owned,
            },
        }),
    }
}

pub fn decode_type_annotated_value_json(
    json: TypeAnnotatedValue,
) -> Result<golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue, GolemError> {
    let typ = analysed_type_client_to_model(&json.typ);
    golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue::parse_with_type(
        &json.value,
        &typ,
    )
    .map_err(|err| {
        GolemError(format!(
            "Invalid type-annotated JSON value: {}",
            err.join(", ")
        ))
    })
}

pub fn encode_type_annotated_value_json(
    value: golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue,
) -> Result<TypeAnnotatedValue, GolemError> {
    let typ: golem_wasm_ast::analysis::AnalysedType = (&value).try_into().map_err(GolemError)?;
    let value = value.to_json_value();

    Ok(TypeAnnotatedValue {
        typ: analysed_type_model_to_client(&typ),
        value,
    })
}
