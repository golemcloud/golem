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

use http::Uri;
use shadow_rs::shadow;
use std::convert::Infallible;
use std::fmt;
use std::fmt::{Display, Formatter};

pub mod base_model;

#[cfg(not(feature = "full"))]
pub mod model {
    pub use crate::base_model::*;
}

#[cfg(feature = "full")]
pub mod cache;
#[cfg(feature = "full")]
pub mod config;
#[cfg(feature = "full")]
pub mod json_yaml;
#[cfg(feature = "full")]
pub mod metrics;
#[cfg(feature = "full")]
pub mod model;
#[cfg(feature = "full")]
pub mod one_shot;
#[cfg(feature = "full")]
pub mod poem;
#[cfg(feature = "full")]
pub mod read_only_lock;
#[cfg(feature = "full")]
pub mod redis;
#[cfg(feature = "full")]
pub mod retriable_error;
#[cfg(feature = "full")]
pub mod retries;
#[cfg(feature = "full")]
pub mod serialization;
#[cfg(feature = "full")]
pub mod tracing;
#[cfg(feature = "full")]
pub mod virtual_exports;

mod macros;

#[cfg(test)]
test_r::enable!();

shadow!(build);

pub fn golem_version() -> &'static str {
    if build::PKG_VERSION != "0.0.0" {
        build::PKG_VERSION
    } else {
        build::GIT_DESCRIBE_TAGS
            .strip_prefix("golem-rust-v")
            .unwrap_or(build::GIT_DESCRIBE_TAGS)
    }
}

/// Trait to convert a value to a string which is safe to return through a public API.
pub trait SafeDisplay {
    fn to_safe_string(&self) -> String;

    fn to_safe_string_indented(&self) -> String {
        let result = self.to_safe_string();
        result
            .lines()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub struct SafeString(String);

impl SafeDisplay for SafeString {
    fn to_safe_string(&self) -> String {
        self.0.clone()
    }
}

impl Display for SafeString {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn safe(value: String) -> impl SafeDisplay {
    SafeString(value)
}

pub fn widen_infallible<T>(_inf: Infallible) -> T {
    panic!("impossible")
}

impl SafeDisplay for () {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

pub trait IntoAnyhow {
    /// Direct conversion to anyhow::Error. This is preferred over going through the blanket Into<anyhow::Error> impl for std::err::Error,
    /// as it can preserve more information depending on the implementor.
    /// Can be removed when specialization is stable or std::err::Error has backtraces.
    fn into_anyhow(self) -> anyhow::Error;
}

pub fn grpc_uri(host: &String, port: u16, tls: bool) -> Uri {
    let scheme = if tls { "https" } else { "http" };

    Uri::builder()
        .scheme(scheme)
        .authority(format!("{}:{}", host, port).as_str())
        .path_and_query("/")
        .build()
        .expect("Failed to build URI")
}
