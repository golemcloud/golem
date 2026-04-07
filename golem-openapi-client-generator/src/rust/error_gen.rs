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

        pub trait ErrorInfo {
            fn service_name() -> &'static str;
            fn status_code(&self) -> u16;
            fn messages(&self) -> Vec<String>;
            fn message(&self) -> String {
                self.messages().join("\n")
            }
            fn code(&self) -> Option<&str>;
            fn is_status_code(&self, status_code: u16) -> bool {
                self.status_code() == status_code
            }
            fn is_success(&self) -> bool {
                (200..300).contains(&self.status_code())
            }
            fn is_client_error(&self) -> bool {
                (400..500).contains(&self.status_code())
            }
            fn is_server_error(&self) -> bool {
                (500..600).contains(&self.status_code())
            }
            fn is_not_found(&self) -> bool {
                self.is_status_code(404)
            }
            fn has_code(&self, code: &str) -> bool {
                self.code() == Some(code)
            }
        }

        #[derive(Debug, thiserror::Error)]
        pub enum Error<T> {
            #[error("{0}")]
            Item(T),
            #[error("Client error: {0}")]
            Reqwest(reqwest::Error),
            #[error("Middleware error: {0}")]
            Middleware(reqwest_middleware::Error),
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

            pub fn status_code(&self) -> Option<u16>
            where
                T: ErrorInfo,
            {
                match self {
                    Error::Item(item) => Some(item.status_code()),
                    Error::Unexpected { code, .. } => Some(*code),
                    _ => None,
                }
            }

            pub fn messages(&self) -> Option<Vec<String>>
            where
                T: ErrorInfo,
            {
                match self {
                    Error::Item(item) => Some(item.messages()),
                    _ => None,
                }
            }

            pub fn message(&self) -> Option<String>
            where
                T: ErrorInfo,
            {
                self.messages().map(|messages| messages.join("\n"))
            }

            pub fn code(&self) -> Option<&str>
            where
                T: ErrorInfo,
            {
                match self {
                    Error::Item(item) => item.code(),
                    _ => None,
                }
            }

            pub fn is_status_code(&self, status_code: u16) -> bool
            where
                T: ErrorInfo,
            {
                self.status_code() == Some(status_code)
            }

            pub fn is_success(&self) -> bool
            where
                T: ErrorInfo,
            {
                self.status_code().is_some_and(|code| (200..300).contains(&code))
            }

            pub fn is_client_error(&self) -> bool
            where
                T: ErrorInfo,
            {
                self.status_code().is_some_and(|code| (400..500).contains(&code))
            }

            pub fn is_server_error(&self) -> bool
            where
                T: ErrorInfo,
            {
                self.status_code().is_some_and(|code| (500..600).contains(&code))
            }

            pub fn is_not_found(&self) -> bool
            where
                T: ErrorInfo,
            {
                self.is_status_code(404)
            }

            pub fn has_code(&self, code: &str) -> bool
            where
                T: ErrorInfo,
            {
                self.code() == Some(code)
            }
        }

        impl<T> From<reqwest::Error> for Error<T> {
            fn from(value: reqwest::Error) -> Self {
                Error::Reqwest(value)
            }
        }

        impl<T> From<reqwest_middleware::Error> for Error<T> {
            fn from(value: reqwest_middleware::Error) -> Self {
                Error::Middleware(value)
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
            exports: vec!["ErrorInfo".to_string(), "Error".to_string()],
        },
        code: code.to_string(),
    }
}
