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

use std::fmt;
use std::fmt::{Display, Formatter};

pub mod cache;
pub mod client;
pub mod config;

pub mod app;
pub mod golem_version;
pub mod grpc;
pub mod log;
pub mod metrics;
pub mod model;
pub mod newtype;
pub mod redis;
pub mod retriable_error;
pub mod retries;
pub mod serialization;
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
