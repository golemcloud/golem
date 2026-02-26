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

use serde::Serialize;
use std::fmt::Debug;
use std::hash::Hash;

pub trait Layer {
    type Id: Debug + Eq + Hash + Clone + Serialize;
    type Value: Debug + Default + Clone + Serialize;
    type Selector;
    type AppliedSelection: Debug + Clone + Serialize;
    type ApplyContext;
    type ApplyError;

    fn id(&self) -> &Self::Id;

    fn parent_layers(&self) -> &[Self::Id];

    fn apply_onto_parent(
        &self,
        ctx: &Self::ApplyContext,
        selector: &Self::Selector,
        value: &mut Self::Value,
    ) -> Result<(), Self::ApplyError>;
}
