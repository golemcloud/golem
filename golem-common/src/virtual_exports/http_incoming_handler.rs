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

use bytes::Bytes;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedInstance};
use golem_wasm_rpc::Value;
use lazy_static::lazy_static;
use semver::Version;

// The following wit is modelled here:
//
// type fields = list<tuple<string, list<u8>>>;
// type body = list<u8>;
//
// record body-and-trailers {
//   body: body,
//   trailers: option<fields>
// }
//
// record request {
//   uri: string
//   method: method,
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

lazy_static! {
    pub static ref REQUIRED_FUNCTIONS: Vec<rib::ParsedFunctionName> = vec![
        rib::ParsedFunctionName {
            site: rib::ParsedFunctionSite::PackagedInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interface: "incoming-handler".to_string(),
                version: Some(rib::SemVer(Version::new(0, 2, 0)))
            },
            function: rib::ParsedFunctionReference::Function {
                function: "handle".to_string()
            }
        },
        rib::ParsedFunctionName {
            site: rib::ParsedFunctionSite::PackagedInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interface: "incoming-handler".to_string(),
                version: Some(rib::SemVer(Version::new(0, 2, 1)))
            },
            function: rib::ParsedFunctionReference::Function {
                function: "handle".to_string()
            }
        },
        rib::ParsedFunctionName {
            site: rib::ParsedFunctionSite::PackagedInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interface: "incoming-handler".to_string(),
                version: Some(rib::SemVer(Version::new(0, 2, 2)))
            },
            function: rib::ParsedFunctionReference::Function {
                function: "handle".to_string()
            }
        },
        rib::ParsedFunctionName {
            site: rib::ParsedFunctionSite::PackagedInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interface: "incoming-handler".to_string(),
                version: Some(rib::SemVer(Version::new(0, 2, 3)))
            },
            function: rib::ParsedFunctionReference::Function {
                function: "handle".to_string()
            }
        }
    ];
    pub static ref PARSED_FUNCTION_NAME: rib::ParsedFunctionName = rib::ParsedFunctionName {
        site: rib::ParsedFunctionSite::PackagedInterface {
            namespace: "golem".to_string(),
            package: "http".to_string(),
            interface: "incoming-handler".to_string(),
            version: None
        },
        function: rib::ParsedFunctionReference::Function {
            function: "handle".to_string()
        }
    };
    pub static ref ANALYZED_FUNCTION_PARAMETERS: Vec<golem_wasm_ast::analysis::AnalysedFunctionParameter> = {
        use golem_wasm_ast::analysis::*;
        vec![AnalysedFunctionParameter {
            name: "request".to_string(),
            typ: IncomingHttpRequest::analysed_type(),
        }]
    };
    pub static ref ANALYZED_FUNCTION_RESULTS: Vec<golem_wasm_ast::analysis::AnalysedFunctionResult> = {
        use golem_wasm_ast::analysis::*;
        vec![AnalysedFunctionResult {
            name: None,
            typ: HttpResponse::analysed_type(),
        }]
    };
    pub static ref ANALYZED_FUNCTION: AnalysedFunction = {
        use golem_wasm_ast::analysis::*;

        AnalysedFunction {
            name: "handle".to_string(),
            parameters: ANALYZED_FUNCTION_PARAMETERS.clone(),
            results: ANALYZED_FUNCTION_RESULTS.clone(),
        }
    };
    pub static ref ANALYZED_EXPORT: AnalysedExport = AnalysedExport::Instance(AnalysedInstance {
        name: "golem:http/incoming-handler".to_string(),
        functions: vec![ANALYZED_FUNCTION.clone()]
    });
}

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
                    name: "Custom".to_string(),
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
            0 => Ok(HttpMethod::GET),
            1 => Ok(HttpMethod::HEAD),
            2 => Ok(HttpMethod::POST),
            3 => Ok(HttpMethod::PUT),
            4 => Ok(HttpMethod::DELETE),
            5 => Ok(HttpMethod::CONNECT),
            6 => Ok(HttpMethod::OPTIONS),
            7 => Ok(HttpMethod::TRACE),
            8 => Ok(HttpMethod::PATCH),
            9 => {
                let value = case_value.as_ref().ok_or("no case_value provided")?;
                let custom_method =
                    extract!(*value.clone(), Value::String(inner), inner, "not a string")?;
                Ok(HttpMethod::Custom(custom_method))
            }
            _ => Err("unknown case")?,
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
    pub uri: String,
    pub method: HttpMethod,
    pub headers: HttpFields,
    pub body: Option<HttpBodyAndTrailers>,
}

impl IncomingHttpRequest {
    pub fn analysed_type() -> AnalysedType {
        use golem_wasm_ast::analysis::*;

        AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "uri".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "method".to_string(),
                    typ: HttpMethod::analyzed_type(),
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

        if record_values.len() != 4 {
            Err("wrong length of record data")?;
        };

        let uri = extract!(
            record_values[0].clone(),
            Value::String(inner),
            inner,
            "not a string"
        )?;
        let method = HttpMethod::from_value(&record_values[1])?;
        let headers = HttpFields::from_value(&record_values[2])?;
        let body = extract!(
            &record_values[3],
            Value::Option(inner),
            match inner {
                Some(v) => Some(HttpBodyAndTrailers::from_value(v)?),
                None => None,
            },
            "not an option"
        )?;

        Ok(IncomingHttpRequest {
            uri,
            method,
            headers,
            body,
        })
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

    pub fn to_value(self) -> Value {
        let converted_status: Value = Value::U16(self.status);
        let converted_headers: Value = self.headers.to_value();
        let converted_body: Value = Value::Option(self.body.map(|b| Box::new(b.to_value())));

        Value::Record(vec![converted_status, converted_headers, converted_body])
    }
}
