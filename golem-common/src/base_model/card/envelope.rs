// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::monomorphization::{
    AgentPermissionMonomorphizationContext, resolve_permissions_for_agent_context,
};
use super::recipient::RecipientPattern;
use super::{PermissionPattern, PolymorphicPermissionPattern};
use crate::model::account::AccountEmail;
use crate::model::agent::AgentTypeName;
use crate::model::application::ApplicationName;
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentName;

pub fn permission_envelopes_for_recipient_patterns(
    permissions: &[PolymorphicPermissionPattern],
) -> Result<Vec<PermissionPattern>, String> {
    let mut result = Vec::new();
    for permission in permissions {
        let context = recipient_envelope_context(permission.recipient());
        result.append(&mut resolve_permissions_for_agent_context(
            std::slice::from_ref(permission),
            &context,
        ));
    }
    Ok(result)
}

fn recipient_envelope_context(
    recipient: &RecipientPattern,
) -> AgentPermissionMonomorphizationContext {
    let wildcard_account = || AccountEmail::new("*");
    let wildcard_application = || ApplicationName("*".to_string());
    let wildcard_environment = || EnvironmentName("*".to_string());
    let wildcard_component = || ComponentName("*".to_string());
    let wildcard_agent_type = || AgentTypeName("*".to_string());

    match recipient {
        RecipientPattern::Any => AgentPermissionMonomorphizationContext {
            account: wildcard_account(),
            application: wildcard_application(),
            environment: wildcard_environment(),
            component: wildcard_component(),
            agent_name: "*".to_string(),
            agent_type: wildcard_agent_type(),
        },
        RecipientPattern::Account { account }
        | RecipientPattern::AccountEnvironments { account }
        | RecipientPattern::AccountAgents { account } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: wildcard_application(),
            environment: wildcard_environment(),
            component: wildcard_component(),
            agent_name: "*".to_string(),
            agent_type: wildcard_agent_type(),
        },
        RecipientPattern::ApplicationEnvironments {
            account,
            application,
        }
        | RecipientPattern::ApplicationAgents {
            account,
            application,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: wildcard_environment(),
            component: wildcard_component(),
            agent_name: "*".to_string(),
            agent_type: wildcard_agent_type(),
        },
        RecipientPattern::Environment {
            account,
            application,
            environment,
        }
        | RecipientPattern::EnvironmentAgents {
            account,
            application,
            environment,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: environment.clone(),
            component: wildcard_component(),
            agent_name: "*".to_string(),
            agent_type: wildcard_agent_type(),
        },
        RecipientPattern::ComponentAgents {
            account,
            application,
            environment,
            component,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: environment.clone(),
            component: component.clone(),
            agent_name: "*".to_string(),
            agent_type: wildcard_agent_type(),
        },
        RecipientPattern::Agent {
            account,
            application,
            environment,
            component,
            agent_type,
        } => AgentPermissionMonomorphizationContext {
            account: account.clone(),
            application: application.clone(),
            environment: environment.clone(),
            component: component.clone(),
            agent_name: format!("{}(*)", agent_type.0),
            agent_type: agent_type.clone(),
        },
    }
}
