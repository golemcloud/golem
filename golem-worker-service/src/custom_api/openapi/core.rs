// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour, RichRouteSecurity};
use golem_common::base_model::agent::{AgentMethod, AgentType};
use golem_common::model::agent::AgentConstructor;
use golem_service_base::custom_api::{PathSegment, RequestBodySchema, SecuritySchemeDetails};
use std::sync::Arc;

pub trait HttpApiRoute {
    fn security_scheme_missing(&self) -> bool;
    fn security_scheme(&self) -> Option<Arc<SecuritySchemeDetails>>;
    fn method(&self) -> &str;
    fn path(&self) -> &Vec<PathSegment>;
    fn binding(&self) -> &RichRouteBehaviour;
    fn request_body_schema(&self) -> &RequestBodySchema;
    fn associated_agent_method(&self) -> Option<&AgentMethod>;
    fn agent_constructor(&self) -> &AgentConstructor;
}

pub struct RichCompiledRouteWithAgentType<'a> {
    pub agent_type: &'a AgentType,
    pub details: &'a RichCompiledRoute,
}

impl<'a> HttpApiRoute for RichCompiledRouteWithAgentType<'a> {
    fn security_scheme_missing(&self) -> bool {
        false
    }
    fn security_scheme(&self) -> Option<Arc<SecuritySchemeDetails>> {
        match &self.details.security {
            RichRouteSecurity::SecurityScheme(details) => Some(details.security_scheme.clone()),
            _ => None,
        }
    }
    fn method(&self) -> &str {
        self.details.method.as_str()
    }
    fn path(&self) -> &Vec<PathSegment> {
        &self.details.path
    }
    fn binding(&self) -> &RichRouteBehaviour {
        &self.details.behavior
    }
    fn request_body_schema(&self) -> &RequestBodySchema {
        &self.details.body
    }

    fn associated_agent_method(&self) -> Option<&AgentMethod> {
        match &self.details.behavior {
            RichRouteBehaviour::CallAgent(call_agent_behaviour) => {
                let method_name = &call_agent_behaviour.method_name;
                self.agent_type
                    .methods
                    .iter()
                    .find(|m| m.name == *method_name)
            }
            _ => None,
        }
    }

    fn agent_constructor(&self) -> &AgentConstructor {
        &self.agent_type.constructor
    }
}
