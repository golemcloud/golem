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

use golem_common::SafeDisplay;
use golem_service_base::model::Component;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

// TODO; This is more specific to specific protocol validations
// There should be a separate validator for worker binding as it is a common to validation to all protocols
pub trait ApiDefinitionValidatorService<ApiDefinition, E> {
    fn validate(
        &self,
        api: &ApiDefinition,
        components: &[Component],
    ) -> Result<(), ValidationErrors<E>>;
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, thiserror::Error)]
pub struct ValidationErrors<E> {
    pub errors: Vec<E>,
}

impl<E: Display> Display for ValidationErrors<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Validation errors: {}",
            self.errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl<E: SafeDisplay> SafeDisplay for ValidationErrors<E> {
    fn to_safe_string(&self) -> String {
        self.errors
            .iter()
            .map(|e| e.to_safe_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}
