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

use super::RouteBehaviour;
use super::SecuritySchemeDetails;
use super::{CompiledRoute, CompiledRoutes};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::security_scheme::SecuritySchemeName;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::collections::HashMap;
use std::ops::Deref;

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::SecuritySchemeDetails>
    for SecuritySchemeDetails
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::SecuritySchemeDetails,
    ) -> Result<Self, Self::Error> {
        let id = value
            .id
            .ok_or_else(|| "id field missing".to_string())?
            .try_into()?;

        let provider_type = value
            .provider()
            .try_into()
            .map_err(|e| format!("invalid provider: {}", e))?;

        let name = SecuritySchemeName(value.name);

        let client_id = ClientId::new(value.client_id);

        let client_secret = ClientSecret::new(value.client_secret);

        let redirect_url = RedirectUrl::new(value.redirect_url)
            .map_err(|e| format!("Failed parsing redirect url: {e}"))?;

        let scopes = value.scopes.into_iter().map(Scope::new).collect::<Vec<_>>();

        Ok(Self {
            id,
            name,
            provider_type,
            client_id,
            client_secret,
            redirect_url,
            scopes,
        })
    }
}

impl From<SecuritySchemeDetails>
    for golem_api_grpc::proto::golem::apidefinition::SecuritySchemeDetails
{
    fn from(value: SecuritySchemeDetails) -> Self {
        Self {
            id: Some(value.id.into()),
            name: value.name.0,
            provider: golem_api_grpc::proto::golem::apidefinition::Provider::from(
                value.provider_type,
            )
            .into(),
            client_id: value.client_id.deref().clone(),
            client_secret: value.client_secret.secret().clone(),
            redirect_url: value.redirect_url.deref().clone(),
            scopes: value.scopes.iter().map(|s| s.deref().clone()).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledRoutes> for CompiledRoutes {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledRoutes,
    ) -> Result<Self, Self::Error> {
        let account_id = value
            .account_id
            .ok_or("Missing field: account_id")?
            .try_into()?;
        let environment_id = value
            .environment_id
            .ok_or("Missing field: environment_id")?
            .try_into()?;

        let mut security_schemes = HashMap::new();
        for scheme in value.security_schemes {
            let scheme: SecuritySchemeDetails = scheme.try_into()?;
            security_schemes.insert(scheme.id, scheme);
        }

        let mut routes = Vec::new();
        for route in value.compiled_routes {
            routes.push(route.try_into()?);
        }

        Ok(Self {
            account_id,
            environment_id,
            deployment_revision: value.deployment_revision.try_into()?,
            security_schemes,
            routes,
        })
    }
}

impl From<CompiledRoutes> for golem_api_grpc::proto::golem::apidefinition::CompiledRoutes {
    fn from(value: CompiledRoutes) -> Self {
        Self {
            account_id: Some(value.account_id.into()),
            environment_id: Some(value.environment_id.into()),
            deployment_revision: value.deployment_revision.into(),
            security_schemes: value
                .security_schemes
                .into_values()
                .map(|v| v.into())
                .collect(),
            compiled_routes: value.routes.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledRoute> for CompiledRoute {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledRoute,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            method: value.method.ok_or("Missing field: method")?.try_into()?,
            path: value
                .path
                .into_iter()
                .map(|p| p.try_into())
                .collect::<Result<Vec<_>, _>>()?,
            header_vars: value
                .header_vars
                .into_iter()
                .map(|p| p.try_into())
                .collect::<Result<Vec<_>, _>>()?,
            query_vars: value
                .query_vars
                .into_iter()
                .map(|p| p.try_into())
                .collect::<Result<Vec<_>, _>>()?,
            behavior: value
                .behaviour
                .ok_or("Missing field: behaviour")?
                .try_into()?,
            security_scheme: value.security_scheme_id.map(|v| v.try_into()).transpose()?,
            cors: value
                .cors_options
                .ok_or("Missing field: cors_options")?
                .try_into()?,
        })
    }
}

impl From<CompiledRoute> for golem_api_grpc::proto::golem::apidefinition::CompiledRoute {
    fn from(value: CompiledRoute) -> Self {
        Self {
            method: Some(value.method.into()),
            path: value.path.into_iter().map(|v| v.into()).collect(),
            header_vars: value.header_vars.into_iter().map(|v| v.into()).collect(),
            query_vars: value.query_vars.into_iter().map(|v| v.into()).collect(),
            behaviour: value.behavior.into(),
            security_scheme_id: value.security_scheme.map(Into::into),
            cors_options: Some(value.cors.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::RouteBehaviour> for RouteBehaviour {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::RouteBehaviour,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::apidefinition::route_behaviour::Behavior;

        match value.behavior.ok_or("RouteBehaviour.behavior missing")? {
            Behavior::CallAgent(agent) => Ok(RouteBehaviour::CallAgent {
                component_id: agent
                    .component_id
                    .ok_or("Missing field: component_id")?
                    .try_into()?,
                component_revision: agent.component_revision.try_into()?,
                agent_type: AgentTypeName(agent.agent_type),
                method_name: agent.method_name,
                input_schema: agent
                    .input_schema
                    .ok_or("Missing field: input_schema")?
                    .try_into()?,
                output_schema: agent
                    .output_schema
                    .ok_or("Missing field: output_schema")?
                    .try_into()?,
            }),
            Behavior::ServeSwaggerUi(_) => Ok(RouteBehaviour::ServeSwaggerUi),
            Behavior::HandleWebhookCallback(_) => Ok(RouteBehaviour::HandleWebhookCallback),
        }
    }
}

impl From<RouteBehaviour> for Option<golem_api_grpc::proto::golem::apidefinition::RouteBehaviour> {
    fn from(value: RouteBehaviour) -> Self {
        use golem_api_grpc::proto::golem::apidefinition::route_behaviour::Behavior;

        Some(match value {
            RouteBehaviour::CallAgent {
                component_id,
                component_revision,
                agent_type,
                method_name,
                input_schema,
                output_schema,
            } => golem_api_grpc::proto::golem::apidefinition::RouteBehaviour {
                behavior: Some(Behavior::CallAgent(
                    golem_api_grpc::proto::golem::apidefinition::CallAgent {
                        component_id: Some(component_id.into()),
                        component_revision: component_revision.get(),
                        agent_type: agent_type.0,
                        method_name,
                        input_schema: Some(input_schema.into()),
                        output_schema: Some(output_schema.into()),
                    },
                )),
            },
            RouteBehaviour::ServeSwaggerUi => {
                golem_api_grpc::proto::golem::apidefinition::RouteBehaviour {
                    behavior: Some(Behavior::ServeSwaggerUi(
                        golem_api_grpc::proto::golem::common::Empty {},
                    )),
                }
            }
            RouteBehaviour::HandleWebhookCallback => {
                golem_api_grpc::proto::golem::apidefinition::RouteBehaviour {
                    behavior: Some(Behavior::HandleWebhookCallback(
                        golem_api_grpc::proto::golem::common::Empty {},
                    )),
                }
            }
        })
    }
}
