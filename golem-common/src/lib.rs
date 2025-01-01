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

use std::fmt;
use std::fmt::{Display, Formatter};

#[cfg(feature = "tokio")]
pub mod cache;

#[cfg(feature = "protobuf")]
pub mod client;

#[cfg(feature = "config")]
pub mod config;

pub mod golem_version;

#[cfg(feature = "protobuf")]
pub mod grpc;

#[cfg(feature = "poem")]
pub mod json_yaml;

#[cfg(feature = "observability")]
pub mod metrics;

pub mod model;
pub mod newtype;

#[cfg(feature = "redis")]
pub mod redis;

#[cfg(feature = "sql")]
pub mod repo;

pub mod retriable_error;

#[cfg(feature = "tokio")]
pub mod retries;

pub mod serialization;

#[cfg(feature = "observability")]
pub mod tracing;

pub mod uri;

#[cfg(test)]
test_r::enable!();

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
