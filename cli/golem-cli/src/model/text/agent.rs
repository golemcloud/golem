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

use crate::model::agent::view::AgentTypeView;
use crate::model::text::fmt::{
    format_message_highlight, log_table, FieldsBuilder, MessageWithFields, TextView,
};
use golem_common::model::agent::DeployedRegisteredAgentType;

impl MessageWithFields for AgentTypeView {
    fn message(&self) -> String {
        format!(
            "Got deployed agent type: {} ",
            format_message_highlight(&self.agent_type)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields.field("Agent type", &self.agent_type);
        fields.field("Constructor", &self.constructor);
        fields.field("Description", &self.description);

        fields.build()
    }
}

impl From<&DeployedRegisteredAgentType> for AgentTypeView {
    fn from(value: &DeployedRegisteredAgentType) -> Self {
        AgentTypeView::new(value, true)
    }
}

impl TextView for Vec<DeployedRegisteredAgentType> {
    fn log(&self) {
        log_table::<_, AgentTypeView>(self);
    }
}
