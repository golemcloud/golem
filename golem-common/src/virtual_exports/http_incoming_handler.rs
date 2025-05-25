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

use bytes::Bytes;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedInstance};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::Value;
use std::sync::LazyLock;

// The following wit is modelled here:
//
// type fields = list<tuple<string, list<u8>>>;
// type body = list<u8>;
//
// variant method {
//   get,
//   head,
//   post,
//   put,
//   delete,
//   connect,
//   options,
//   trace,
//   patch,
//   custom(string)
// }
//
// variant scheme {
//    HTTP,
//    HTTPS,
//    custom(string)
//  }
//
// record body-and-trailers {
//   body: body,
//   trailers: option<fields>
// }
//
// record request {
//   method: method,
//   scheme: scheme,
//   authority: string,
//   path-and-query: string,
//   headers: fields,
//   body-and-trailers: option<body-and-trailers>
// }
//
// record response {
//   status: status-code,
//   headers: fields,
//   body: option<body-and-trailers>
// }
//
// handle: func(request: request) -> response;
//

pub static PARSED_FUNCTION_NAME: LazyLock<rib::ParsedFunctionName> =
    LazyLock::new(|| rib::ParsedFunctionName {
        site: rib::ParsedFunctionSite::PackagedInterface {
            namespace: "golem".to_string(),
            package: "http".to_string(),
            interface: "incoming-handler".to_string(),
            version: None,
        },
        function: rib::ParsedFunctionReference::Function {
            function: "handle".to_string(),
        },
    });

pub static ANALYZED_FUNCTION_PARAMETERS: LazyLock<
    Vec<golem_wasm_ast::analysis::AnalysedFunctionParameter>,
> = {
    use golem_wasm_ast::analysis::*;
    LazyLock::new(|| {
        vec![AnalysedFunctionParameter {
            name: "request".to_string(),
            typ: IncomingHttpRequest::analysed_type(),
        }]
    })
};

pub static ANALYZED_FUNCTION_RESULTS: LazyLock<
    Vec<golem_wasm_ast::analysis::AnalysedFunctionResult>,
> = {
    use golem_wasm_ast::analysis::*;
    LazyLock::new(|| {
        vec![AnalysedFunctionResult {
            name: None,
            typ: HttpResponse::analysed_type(),
        }]
    })
};

pub static ANALYZED_FUNCTION: LazyLock<AnalysedFunction> = {
    use golem_wasm_ast::analysis::*;

    LazyLock::new(|| AnalysedFunction {
        name: "handle".to_string(),
        parameters: ANALYZED_FUNCTION_PARAMETERS.clone(),
        results: ANALYZED_FUNCTION_RESULTS.clone(),
    })
};

pub const FUNCTION_NAME: &str = "golem:http/incoming-handler.{handle}";

pub static ANALYZED_EXPORT: LazyLock<AnalysedExport> = LazyLock::new(|| {
    AnalysedExport::Instance(AnalysedInstance {
        name: "golem:http/incoming-handler".to_string(),
        functions: vec![ANALYZED_FUNCTION.clone()],
    })
});

pub fn implements_required_interfaces(exports: &[AnalysedExport]) -> bool {
    let compatible_interfaces = [
        "wasi:http/incoming-handler@0.2.0".to_string(),
        "wasi:http/incoming-handler@0.2.1".to_string(),
        "wasi:http/incoming-handler@0.2.2".to_string(),
        "wasi:http/incoming-handler@0.2.3".to_string(),
    ];

    exports.iter().any(|ae| match ae {
        AnalysedExport::Instance(AnalysedInstance { name, .. }) => {
            compatible_interfaces.contains(name)
        }
        _ => false,
    })
}

macro_rules! extract {
    ($expression:expr, $pattern:pat $(if $guard:expr)? $(,)?, $ret:expr, $err:expr) => {
        match $expression {
            $pattern $(if $guard)? => Ok($ret),
            _ => Err($err)
        }
    };
}

pub enum HttpScheme {
    HTTP,
    HTTPS,
    Custom(String),
}

impl HttpScheme {
    pub fn analyzed_type() -> AnalysedType {
        use golem_wasm_ast::analysis::*;
        AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "HTTP".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "HTTPS".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "custom".to_string(),
                    typ: Some(AnalysedType::Str(TypeStr)),
                },
            ],
        })
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let (case_idx, case_value) = extract!(
            value,
            Value::Variant {
                case_idx,
                case_value
            },
            (case_idx, case_value),
            "not a variant"
        )?;

        match case_idx {
            0 => Ok(Self::HTTP),
            1 => Ok(Self::HTTPS),
            2 => {
                let value = case_value.as_ref().ok_or("no case_value provided")?;
                let custom_method =
                    extract!(*value.clone(), Value::String(inner), inner, "not a string")?;
                Ok(Self::Custom(custom_method))
            }
            _ => Err("unknown case")?,
        }
    }

    pub fn to_value(self) -> Value {
        match self {
            Self::HTTP => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            Self::HTTPS => Value::Variant {
                case_idx: 1,
                case_value: None,
            },
            Self::Custom(custom_method) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(Value::String(custom_method))),
            },
        }
    }
}

impl From<http::uri::Scheme> for HttpScheme {
    fn from(value: http::uri::Scheme) -> Self {
        match value {
            well_known if well_known == http::uri::Scheme::HTTP => Self::HTTP,
            well_known if well_known == http::uri::Scheme::HTTPS => Self::HTTPS,
            other => Self::Custom(other.to_string()),
        }
    }
}

pub enum HttpMethod {
    GET,
    HEAD,
    POST,
    PUT,
    DELETE,
    CONNECT,
    OPTIONS,
    TRACE,
    PATCH,
    Custom(String),
}

impl HttpMethod {
    pub fn analyzed_type() -> AnalysedType {
        use golem_wasm_ast::analysis::*;
        AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "GET".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "HEAD".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "POST".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "PUT".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "DELETE".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "CONNECT".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "OPTIONS".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "TRACE".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "PATCH".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "custom".to_string(),
                    typ: Some(AnalysedType::Str(TypeStr)),
                },
            ],
        })
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let (case_idx, case_value) = extract!(
            value,
            Value::Variant {
                case_idx,
                case_value
            },
            (case_idx, case_value),
            "not a variant"
        )?;

        match case_idx {
            0 => Ok(Self::GET),
            1 => Ok(Self::HEAD),
            2 => Ok(Self::POST),
            3 => Ok(Self::PUT),
            4 => Ok(Self::DELETE),
            5 => Ok(Self::CONNECT),
            6 => Ok(Self::OPTIONS),
            7 => Ok(Self::TRACE),
            8 => Ok(Self::PATCH),
            9 => {
                let value = case_value.as_ref().ok_or("no case_value provided")?;
                let custom_method =
                    extract!(*value.clone(), Value::String(inner), inner, "not a string")?;
                Ok(Self::Custom(custom_method))
            }
            _ => Err("unknown case")?,
        }
    }

    pub fn to_value(self) -> Value {
        match self {
            Self::GET => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            Self::HEAD => Value::Variant {
                case_idx: 1,
                case_value: None,
            },
            Self::POST => Value::Variant {
                case_idx: 2,
                case_value: None,
            },
            Self::PUT => Value::Variant {
                case_idx: 3,
                case_value: None,
            },
            Self::DELETE => Value::Variant {
                case_idx: 4,
                case_value: None,
            },
            Self::CONNECT => Value::Variant {
                case_idx: 5,
                case_value: None,
            },
            Self::OPTIONS => Value::Variant {
                case_idx: 6,
                case_value: None,
            },
            Self::TRACE => Value::Variant {
                case_idx: 7,
                case_value: None,
            },
            Self::PATCH => Value::Variant {
                case_idx: 8,
                case_value: None,
            },
            Self::Custom(custom_method) => Value::Variant {
                case_idx: 9,
                case_value: Some(Box::new(Value::String(custom_method))),
            },
        }
    }

    pub fn from_http_method(value: http::Method) -> Self {
        use http::Method as M;

        match value {
            M::GET => Self::GET,
            M::CONNECT => Self::CONNECT,
            M::DELETE => Self::DELETE,
            M::HEAD => Self::HEAD,
            M::OPTIONS => Self::OPTIONS,
            M::PATCH => Self::PATCH,
            M::POST => Self::POST,
            M::PUT => Self::PUT,
            M::TRACE => Self::TRACE,
            other => Self::Custom(other.to_string()),
        }
    }
}

impl TryInto<http::Method> for HttpMethod {
    type Error = http::method::InvalidMethod;

    fn try_into(self) -> Result<http::Method, Self::Error> {
        match self {
            Self::GET => Ok(http::Method::GET),
            Self::HEAD => Ok(http::Method::HEAD),
            Self::POST => Ok(http::Method::POST),
            Self::PUT => Ok(http::Method::PUT),
            Self::DELETE => Ok(http::Method::DELETE),
            Self::CONNECT => Ok(http::Method::CONNECT),
            Self::OPTIONS => Ok(http::Method::OPTIONS),
            Self::TRACE => Ok(http::Method::TRACE),
            Self::PATCH => Ok(http::Method::PATCH),
            Self::Custom(method) => http::Method::from_bytes(method.as_bytes()),
        }
    }
}

pub struct HttpFields(pub Vec<(String, Bytes)>);

impl HttpFields {
    pub fn analyzed_type() -> AnalysedType {
        use golem_wasm_ast::analysis::*;
        AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Tuple(TypeTuple {
                items: vec![
                    AnalysedType::Str(TypeStr),
                    AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::U8(TypeU8)),
                    }),
                ],
            })),
        })
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let mut result = Vec::new();

        let list_values = extract!(value, Value::List(inner), inner, "not a list")?;

        for lv in list_values {
            let tuple_value = extract!(lv, Value::Tuple(inner), inner, "not a tuple")?;

            let (name, values) = extract!(
                tuple_value.as_slice(),
                [Value::String(name), Value::List(values)],
                (name.clone(), values),
                "incompatible types"
            )?;

            let mut result_value = Vec::new();

            for v in values {
                let v = extract!(v, Value::U8(inner), *inner, "not a byte")?;
                result_value.push(v);
            }

            result.push((name, Bytes::from(result_value)));
        }

        Ok(HttpFields(result))
    }

    pub fn to_value(self) -> Value {
        let mut list_values = Vec::new();

        for (name, value) in self.0 {
            let converted_bytes: Vec<Value> = value.into_iter().map(Value::U8).collect::<Vec<_>>();

            list_values.push(Value::Tuple(vec![
                Value::String(name),
                Value::List(converted_bytes),
            ]));
        }
        Value::List(list_values)
    }
}

pub struct HttpBodyContent(pub Bytes);

impl HttpBodyContent {
    pub fn analyzed_type() -> AnalysedType {
        use golem_wasm_ast::analysis::*;
        AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::U8(TypeU8)),
        })
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let mut result = Vec::new();

        let list_values = extract!(value, Value::List(inner), inner, "not a list")?;

        for lv in list_values {
            let byte_value = extract!(lv, Value::U8(inner), *inner, "not a byte")?;
            result.push(byte_value);
        }

        Ok(HttpBodyContent(Bytes::from(result)))
    }

    pub fn to_value(self) -> Value {
        let converted = self.0.into_iter().map(Value::U8).collect::<Vec<_>>();
        Value::List(converted)
    }
}

pub struct HttpBodyAndTrailers {
    pub content: HttpBodyContent,
    pub trailers: Option<HttpFields>,
}

impl HttpBodyAndTrailers {
    pub fn analysed_type() -> AnalysedType {
        use golem_wasm_ast::analysis::*;

        AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "content".to_string(),
                    typ: HttpBodyContent::analyzed_type(),
                },
                NameTypePair {
                    name: "trailers".to_string(),
                    typ: AnalysedType::Option(TypeOption {
                        inner: Box::new(HttpFields::analyzed_type()),
                    }),
                },
            ],
        })
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let record_values = extract!(value, Value::Record(inner), inner, "not a record")?;

        if record_values.len() != 2 {
            Err("wrong length of record data")?;
        };

        let content = HttpBodyContent::from_value(&record_values[0])?;
        let trailers = extract!(
            &record_values[1],
            Value::Option(inner),
            match inner {
                Some(inner) => Some(HttpFields::from_value(inner)?),
                None => None,
            },
            "not an option"
        )?;

        Ok(HttpBodyAndTrailers { content, trailers })
    }
    pub fn to_value(self) -> Value {
        let converted_content = self.content.to_value();
        let converted_trailers = Value::Option(self.trailers.map(|t| Box::new(t.to_value())));

        Value::Record(vec![converted_content, converted_trailers])
    }
}

pub struct IncomingHttpRequest {
    pub method: HttpMethod,
    pub scheme: HttpScheme,
    pub authority: String,
    pub path_and_query: String,
    pub headers: HttpFields,
    pub body: Option<HttpBodyAndTrailers>,
}

impl IncomingHttpRequest {
    pub fn analysed_type() -> AnalysedType {
        use golem_wasm_ast::analysis::*;

        AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "method".to_string(),
                    typ: HttpMethod::analyzed_type(),
                },
                NameTypePair {
                    name: "scheme".to_string(),
                    typ: HttpScheme::analyzed_type(),
                },
                NameTypePair {
                    name: "authority".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "path-and-query".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "headers".to_string(),
                    typ: HttpFields::analyzed_type(),
                },
                NameTypePair {
                    name: "body-and-trailers".to_string(),
                    typ: AnalysedType::Option(TypeOption {
                        inner: Box::new(HttpBodyAndTrailers::analysed_type()),
                    }),
                },
            ],
        })
    }

    pub fn from_function_input(inputs: &[Value]) -> Result<Self, String> {
        if inputs.len() != 1 {
            Err("invalid number of inputs")?;
        };
        Self::from_value(&inputs[0])
            .map_err(|e| format!("Failed parsing input as http request: {e}"))
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let record_values = extract!(value, Value::Record(inner), inner, "not a record")?;

        if record_values.len() != 6 {
            Err("wrong length of record data")?;
        };

        let method = HttpMethod::from_value(&record_values[0])?;
        let scheme = HttpScheme::from_value(&record_values[1])?;
        let authority = extract!(
            record_values[2].clone(),
            Value::String(inner),
            inner,
            "not a string"
        )?;
        let path_and_query = extract!(
            record_values[3].clone(),
            Value::String(inner),
            inner,
            "not a string"
        )?;
        let headers = HttpFields::from_value(&record_values[4])?;
        let body = extract!(
            &record_values[5],
            Value::Option(inner),
            match inner {
                Some(v) => Some(HttpBodyAndTrailers::from_value(v)?),
                None => None,
            },
            "not an option"
        )?;

        Ok(IncomingHttpRequest {
            method,
            scheme,
            authority,
            path_and_query,
            headers,
            body,
        })
    }

    pub fn to_value(self) -> Value {
        Value::Record(vec![
            self.method.to_value(),
            self.scheme.to_value(),
            Value::String(self.authority),
            Value::String(self.path_and_query),
            self.headers.to_value(),
            Value::Option(self.body.map(|b| Box::new(b.to_value()))),
        ])
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: HttpFields,
    pub body: Option<HttpBodyAndTrailers>,
}

impl HttpResponse {
    pub fn analysed_type() -> AnalysedType {
        use golem_wasm_ast::analysis::*;

        AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::U16(TypeU16),
                },
                NameTypePair {
                    name: "headers".to_string(),
                    typ: HttpFields::analyzed_type(),
                },
                NameTypePair {
                    name: "body-and-trailers".to_string(),
                    typ: AnalysedType::Option(TypeOption {
                        inner: Box::new(HttpBodyAndTrailers::analysed_type()),
                    }),
                },
            ],
        })
    }

    pub fn from_value(value: Value) -> Result<Self, String> {
        let record_values = extract!(value, Value::Record(inner), inner, "not a record")?;

        if record_values.len() != 3 {
            Err("wrong length of record data")?;
        };

        let status = extract!(
            record_values[0].clone(),
            Value::U16(inner),
            inner,
            "not a u16"
        )?;

        let headers = HttpFields::from_value(&record_values[1])?;

        let body = extract!(
            &record_values[2],
            Value::Option(inner),
            inner.as_ref(),
            "not an option"
        )?;
        let body = if let Some(b) = body {
            Some(HttpBodyAndTrailers::from_value(b)?)
        } else {
            None
        };

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    pub fn from_function_output(output: TypeAnnotatedValue) -> Result<Self, String> {
        let value: Value = output.try_into()?;

        let mut tuple_values = extract!(value, Value::Tuple(inner), inner, "not a tuple")?;

        if tuple_values.len() != 1 {
            Err("unexpected number of outputs")?
        };

        Self::from_value(tuple_values.remove(0))
    }

    pub fn to_value(self) -> Value {
        let converted_status: Value = Value::U16(self.status);
        let converted_headers: Value = self.headers.to_value();
        let converted_body: Value = Value::Option(self.body.map(|b| Box::new(b.to_value())));

        Value::Record(vec![converted_status, converted_headers, converted_body])
    }
}
