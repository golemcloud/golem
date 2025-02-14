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

use golem_common::model::component::DefaultComponentOwner;
use golem_common::model::ComponentId;

use crate::error::GolemError;


/// Used to resolve a ComponentId from a user-supplied string.
pub trait ComponentResolver<ComponentOwner>: Send + Sync {
    /// Resolve a component given a user provided string. The syntax of the provided string is allowed to vary between implementations.
    /// `context` contains details about the current component in which the resolution is taking place.
    fn resolve_component(&self, component_reference: String, context: ComponentOwner) -> Result<Option<ComponentId>, GolemError>;
}

pub struct DefaultComponentResolver;

/// Only supports resolving components based on the component name.
impl ComponentResolver<DefaultComponentOwner> for DefaultComponentResolver {
    fn resolve_component(&self, component_reference: String, context: DefaultComponentOwner) -> Result<Option<ComponentId>, GolemError> {
        todo!()
    }
}
