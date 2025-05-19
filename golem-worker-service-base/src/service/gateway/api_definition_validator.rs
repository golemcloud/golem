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

use crate::gateway_api_definition::ApiDefinitionId;
use golem_common::SafeDisplay;
use golem_service_base::model::Component;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

// TODO: This is more specific to specific protocol validations
// There should be a separate validator for worker binding as it is a common to validation to all protocols
pub trait ApiDefinitionValidatorService<ApiDefinition> {
    fn validate(
        &self,
        api: &ApiDefinition,
        components: &[Component],
    ) -> Result<(), ValidationErrors>;
    fn validate_name(&self, id: &ApiDefinitionId) -> Result<(), ValidationErrors>;
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, thiserror::Error)]
pub struct ValidationErrors {
    pub errors: Vec<String>,
}

impl Display for ValidationErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Validation errors: {}",
            self.errors
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl SafeDisplay for ValidationErrors {
    fn to_safe_string(&self) -> String {
        self.errors
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}
