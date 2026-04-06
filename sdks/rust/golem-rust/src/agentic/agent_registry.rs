// Copyright 2024-2026 Golem Cloud
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

use crate::agentic::extended_agent_type::ExtendedAgentType;
use crate::agentic::{EnrichedElementSchema, ExtendedDataSchema, Principal};
use crate::{
    agentic::{ResolvedAgent, agent_initiator::AgentInitiator},
    golem_agentic::exports::golem::agent::guest::AgentType,
};
use golem_wasm::golem_core_1_5_x::types::parse_uuid;
use golem_wasm::{AgentId, ComponentId};
use std::rc::Rc;
use std::{cell::RefCell, future::Future};
use std::{collections::HashMap, sync::Arc};
use wstd::runtime::block_on;

#[derive(Default)]
pub struct State {
    pub agent_types: RefCell<AgentTypes>,
    pub agent_instance: RefCell<AgentInstance>,
    pub agent_initiators: RefCell<AgentInitiators>,
    pub initialized_principal: RefCell<Option<Principal>>,
}

#[derive(Default)]
pub struct AgentTypes {
    pub agent_types: HashMap<AgentTypeName, ExtendedAgentType>,
}

static mut STATE: Option<State> = None;

#[allow(static_mut_refs)]
pub fn get_state() -> &'static State {
    unsafe {
        if STATE.is_none() {
            STATE = Some(State::default());
        }
        STATE.as_ref().unwrap()
    }
}

#[derive(Default)]
pub struct AgentInstance {
    pub resolved_agent: Option<Rc<ResolvedAgent>>,
}

#[derive(Default)]
pub struct AgentInitiators {
    pub agent_initiators: HashMap<AgentTypeName, Arc<dyn AgentInitiator>>,
}

pub fn get_all_agent_types() -> Vec<AgentType> {
    let state = get_state();

    state
        .agent_types
        .borrow()
        .agent_types
        .values()
        .map(|e| e.to_agent_type())
        .collect()
}

pub fn get_enriched_agent_type_by_name(
    agent_type_name: &AgentTypeName,
) -> Option<ExtendedAgentType> {
    let state = get_state();

    state
        .agent_types
        .borrow()
        .agent_types
        .get(agent_type_name)
        .cloned()
}

pub fn get_agent_type_by_name(agent_type_name: &AgentTypeName) -> Option<AgentType> {
    let enriched = get_enriched_agent_type_by_name(agent_type_name);

    enriched.map(|e| e.to_agent_type())
}

pub fn register_principal(principal: &Principal) {
    let state = get_state();

    *state.initialized_principal.borrow_mut() = Some(principal.clone());
}

pub fn get_principal() -> Option<Principal> {
    let state = get_state();

    state.initialized_principal.borrow().clone()
}

pub fn register_agent_type(agent_type_name: AgentTypeName, agent_type: ExtendedAgentType) {
    get_state()
        .agent_types
        .borrow_mut()
        .agent_types
        .insert(agent_type_name, agent_type);
}

pub fn register_agent_initiator(agent_type_name: &str, initiator: Arc<dyn AgentInitiator>) {
    let state = get_state();
    let agent_type_name = AgentTypeName(agent_type_name.to_string());

    state
        .agent_initiators
        .borrow_mut()
        .agent_initiators
        .insert(agent_type_name, initiator);
}

pub fn register_agent_instance(resolved_agent: ResolvedAgent) {
    let state = get_state();

    state.agent_instance.borrow_mut().resolved_agent = Some(Rc::new(resolved_agent));
}

// To be used only in agent implementation
pub fn with_agent_instance_async<F, Fut, R>(f: F) -> R
where
    F: FnOnce(Rc<ResolvedAgent>) -> Fut,
    Fut: Future<Output = R>,
{
    let agent_instance = get_state()
        .agent_instance
        .borrow()
        .resolved_agent
        .clone()
        .unwrap();

    block_on(async move { f(agent_instance).await })
}

pub fn with_agent_instance<F, R>(f: F) -> R
where
    F: FnOnce(&ResolvedAgent) -> R,
{
    let agent_instance = get_state()
        .agent_instance
        .borrow()
        .resolved_agent
        .clone()
        .unwrap();

    f(agent_instance.as_ref())
}

pub fn get_agent_id() -> AgentId {
    let env_vars: HashMap<String, String> =
        HashMap::from_iter(wasip2::cli::environment::get_environment());
    let raw_agent_id = env_vars
        .get("GOLEM_AGENT_ID")
        .expect("Missing GOLEM_AGENT_ID environment variable"); // This is always provided by the Golem runtime
    let raw_component_id = env_vars
        .get("GOLEM_COMPONENT_ID")
        .expect("Missing GOLEM_COMPONENT_ID environment variable");
    AgentId {
        component_id: ComponentId {
            uuid: parse_uuid(raw_component_id).expect("Invalid GOLEM_COMPONENT_ID"),
        },
        agent_id: raw_agent_id.clone(),
    }
}

pub fn get_resolved_agent() -> Option<Rc<ResolvedAgent>> {
    get_state().agent_instance.borrow().resolved_agent.clone()
}

pub fn get_constructor_parameter_type(
    agent_type_name: &AgentTypeName,
    parameter_index: usize,
) -> Option<EnrichedElementSchema> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;

    extract_parameter_schema(&agent_type.constructor.input_schema, parameter_index)
}

pub fn get_method_parameter_type(
    agent_type_name: &AgentTypeName,
    method_name: &str,
    parameter_index: usize,
) -> Option<EnrichedElementSchema> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;

    let method = agent_type.methods.iter().find(|m| m.name == method_name)?;

    extract_parameter_schema(&method.input_schema, parameter_index)
}

pub fn get_constructor_parameter_types(
    agent_type_name: &AgentTypeName,
) -> Option<Vec<EnrichedElementSchema>> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;

    Some(extract_all_parameter_schemas(&agent_type.constructor.input_schema))
}

pub fn get_method_parameter_types(
    agent_type_name: &AgentTypeName,
    method_name: &str,
) -> Option<Vec<EnrichedElementSchema>> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;
    let method = agent_type.methods.iter().find(|m| m.name == method_name)?;

    Some(extract_all_parameter_schemas(&method.input_schema))
}

fn extract_all_parameter_schemas(schema: &ExtendedDataSchema) -> Vec<EnrichedElementSchema> {
    match schema {
        ExtendedDataSchema::Tuple(items) => items.iter().map(|(_, s)| s.clone()).collect(),
        ExtendedDataSchema::Multimodal(items) => items
            .iter()
            .map(|(_, s)| EnrichedElementSchema::ElementSchema(s.clone()))
            .collect(),
    }
}

fn extract_parameter_schema(
    schema: &ExtendedDataSchema,
    parameter_index: usize,
) -> Option<EnrichedElementSchema> {
    match schema {
        ExtendedDataSchema::Tuple(items) => items
            .get(parameter_index)
            .map(|(_, element_schema)| element_schema.clone()),
        ExtendedDataSchema::Multimodal(items) => {
            items.get(parameter_index).map(|(_, element_schema)| {
                EnrichedElementSchema::ElementSchema(element_schema.clone())
            })
        }
    }
}

pub fn with_agent_initiator<F, Fut, R>(f: F, agent_type_name: &AgentTypeName) -> R
where
    F: FnOnce(Arc<dyn AgentInitiator>) -> Fut,
    Fut: Future<Output = R>,
{
    let state = get_state();

    let agent_initiator = state
        .agent_initiators
        .borrow()
        .agent_initiators
        .get(agent_type_name)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "Agent initiator not found for agent type name: {}",
                agent_type_name.0
            )
        });

    block_on(async move { f(agent_initiator).await })
}

#[derive(Eq, Hash, PartialEq)]
pub struct AgentTypeName(pub String);

impl std::borrow::Borrow<str> for AgentTypeName {
    fn borrow(&self) -> &str {
        &self.0
    }
}
