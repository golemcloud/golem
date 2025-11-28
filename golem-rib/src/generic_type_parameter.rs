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

use desert_rust::BinaryCodec;
use std::fmt::Display;

#[derive(Debug, Hash, Clone, PartialEq, Eq, Ord, PartialOrd, BinaryCodec)]
#[desert(evolution())]
pub struct GenericTypeParameter {
    pub value: String,
}

impl Display for GenericTypeParameter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}
