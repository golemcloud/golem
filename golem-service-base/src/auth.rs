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

use golem_common::model::{AccountId, ProjectId};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

pub trait GolemAuthCtx: Send + Sync + Clone + IntoIterator<Item = (String, String)> {}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmptyAuthCtx();

impl Display for EmptyAuthCtx {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "EmptyAuthCtx")
    }
}

impl IntoIterator for EmptyAuthCtx {
    type Item = (String, String);
    type IntoIter = std::iter::Empty<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        std::iter::empty()
    }
}

impl GolemAuthCtx for EmptyAuthCtx {}

pub trait GolemNamespace:
    Send + Sync + Clone + Eq + TryFrom<String, Error = String> + Display + 'static
{
    fn account_id(&self) -> AccountId;
    fn project_id(&self) -> Option<ProjectId>;
}

#[derive(
    Default,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    bincode::Encode,
    bincode::Decode,
    Serialize,
    Deserialize,
)]
pub struct DefaultNamespace();

impl GolemNamespace for DefaultNamespace {
    fn account_id(&self) -> AccountId {
        AccountId::placeholder()
    }
    fn project_id(&self) -> Option<ProjectId> {
        None
    }
}

impl Display for DefaultNamespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "default")
    }
}

impl TryFrom<String> for DefaultNamespace {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.as_str() == "default" {
            Ok(DefaultNamespace::default())
        } else {
            Err("Failed to parse empty namespace".to_string())
        }
    }
}
