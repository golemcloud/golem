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

use shadow_rs::shadow;
use std::convert::Infallible;
use std::fmt;
use std::fmt::{Display, Formatter};

#[cfg(feature = "base-model")]
pub mod base_model;

#[cfg(feature = "tokio")]
pub mod cache;

#[cfg(feature = "protobuf")]
pub mod client;

#[cfg(feature = "config")]
pub mod config;

#[cfg(feature = "protobuf")]
pub mod grpc;

#[cfg(feature = "poem")]
pub mod json_yaml;

#[cfg(feature = "observability")]
pub mod metrics;

#[cfg(feature = "model")]
pub mod model;

#[cfg(any(feature = "model", feature = "base-model"))]
pub mod newtype;

#[cfg(feature = "redis")]
pub mod redis;

#[cfg(feature = "sql")]
pub mod repo;

#[cfg(feature = "tokio")]
pub mod retriable_error;

#[cfg(feature = "tokio")]
pub mod retries;

#[cfg(feature = "serialization")]
pub mod serialization;

#[cfg(feature = "observability")]
pub mod tracing;

#[cfg(feature = "model")]
pub mod virtual_exports;

#[cfg(test)]
test_r::enable!();

shadow!(build);

pub fn golem_version() -> &'static str {
    if build::PKG_VERSION != "0.0.0" {
        build::PKG_VERSION
    } else {
        build::GIT_DESCRIBE_TAGS
    }
}

/// Trait to convert a value to a string which is safe to return through a public API.
pub trait SafeDisplay {
    fn to_safe_string(&self) -> String;
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
