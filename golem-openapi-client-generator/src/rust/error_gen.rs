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

use crate::rust::lib_gen::{Module, ModuleDef, ModuleName};
use indoc::indoc;

pub fn error_gen() -> Module {
    let code = indoc! { r#"
        use bytes::Bytes;

        #[derive(Debug, thiserror::Error)]
        pub enum Error<T> {
            #[error("{0}")]
            Item(T),
            #[error("Client error: {0}")]
            Reqwest(reqwest::Error),
            #[error("Invalid header value: {0}")]
            ReqwestHeader(reqwest::header::InvalidHeaderValue),
            #[error("Deserialization error: {0}")]
            Serde(serde_json::Error),
            #[error("Unexpected response - status {code}, data: {body}", body = "String::from_utf8_lossy(data.as_ref())")]
            Unexpected {
                code: u16,
                data: Bytes,
            }
        }

        impl<T> Error<T> {
            pub fn unexpected(code: u16, data: Bytes) -> Error<T> {
                Error::Unexpected { code, data }
            }
        }

        impl<T> From<reqwest::Error> for Error<T> {
            fn from(value: reqwest::Error) -> Self {
                Error::Reqwest(value)
            }
        }

        impl<T> From<serde_json::Error> for Error<T> {
            fn from(value: serde_json::Error) -> Self {
                Error::Serde(value)
            }
        }

        impl<T> From<reqwest::header::InvalidHeaderValue> for Error<T> {
            fn from(value: reqwest::header::InvalidHeaderValue) -> Self {
                Error::ReqwestHeader(value)
            }
        }
    "#};

    Module {
        def: ModuleDef {
            name: ModuleName::new("error"),
            exports: vec!["Error".to_string()],
        },
        code: code.to_string(),
    }
}
