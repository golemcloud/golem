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

use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::Value;

pub fn generate_value(analysed_tpe: &AnalysedType) -> Value {
    match analysed_tpe {
        AnalysedType::Variant(typed_variant) => {
            let first_case = typed_variant.cases.first();

            if let Some(first_case) = first_case {
                let case_type = &first_case.typ;

                if let Some(case_type) = case_type {
                    let typ = generate_value(case_type);
                    Value::Variant {
                        case_idx: 0,
                        case_value: Some(Box::new(typ)),
                    }
                } else {
                    Value::Variant {
                        case_idx: 0,
                        case_value: None,
                    }
                }
            } else {
                Value::Variant {
                    case_idx: 0,
                    case_value: None,
                }
            }
        }
        AnalysedType::Result(typ) => {
            let ok_type = &typ.ok;
            let err_type = &typ.err;

            match ok_type {
                Some(ok_tpe) => {
                    let ok_value = generate_value(ok_tpe);
                    Value::Result(Ok(Some(Box::new(ok_value))))
                }
                None => match err_type {
                    Some(err_tpe) => {
                        let err_value = generate_value(err_tpe);
                        Value::Result(Err(Some(Box::new(err_value))))
                    }
                    None => Value::Result(Ok(None)),
                },
            }
        }
        AnalysedType::Option(typ) => {
            let inner_type = &typ.inner;
            let inner_value = generate_value(inner_type);

            Value::Option(Some(Box::new(inner_value)))
        }
        AnalysedType::Enum(_) => Value::Enum(0),

        AnalysedType::Flags(flags) => {
            let flag_names = &flags.names;
            let length = flag_names.len();
            let flags = vec![true; length];

            Value::Flags(flags)
        }
        AnalysedType::Record(typed_record) => {
            let fields = &typed_record.fields;
            let mut values = vec![];

            for field in fields {
                let field_type = &field.typ;
                let field_value = generate_value(field_type);

                values.push(field_value);
            }

            Value::Record(values)
        }
        AnalysedType::Tuple(tuple) => {
            let inner_types = &tuple.items;
            let mut values = vec![];

            for inner_type in inner_types {
                let inner_value = generate_value(inner_type);

                values.push(inner_value);
            }

            Value::Tuple(values)
        }
        AnalysedType::List(typ) => {
            let inner_type = &typ.inner;
            let inner_value = generate_value(inner_type);

            let values = vec![inner_value.clone(); 3];

            Value::List(values)
        }
        AnalysedType::Str(_) => Value::String("foo".to_string()),
        AnalysedType::Chr(_) => Value::Char('c'),
        AnalysedType::F64(_) => Value::F64(42.0),
        AnalysedType::F32(_) => Value::F32(42.0),
        AnalysedType::U64(_) => Value::U64(42),
        AnalysedType::S64(_) => Value::S64(42),
        AnalysedType::U32(_) => Value::U32(42),
        AnalysedType::S32(_) => Value::S32(42),
        AnalysedType::U16(_) => Value::U16(42),
        AnalysedType::S16(_) => Value::S16(42),
        AnalysedType::U8(_) => Value::U8(42),
        AnalysedType::S8(_) => Value::S8(42),
        AnalysedType::Bool(_) => Value::Bool(true),
        AnalysedType::Handle(_) => Value::Handle {
            uri: "".to_string(),
            resource_id: 0,
        },
    }
}
