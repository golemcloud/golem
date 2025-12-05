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

use crate::agentic::Agent;
use crate::{
    agentic::agent_initiator::AgentInitiator,
    golem_agentic::{exports::golem::agent::guest::AgentType, golem::agent::common::ElementSchema},
};
use golem_wasm::golem_rpc_0_2_x::types::parse_uuid;
use golem_wasm::{AgentId, ComponentId};
use std::sync::RwLock;
use std::{cell::RefCell, future::Future};
use std::{collections::HashMap, sync::Arc};
use wstd::runtime::block_on;

#[derive(Default)]
pub struct State {
    pub agent_types: RefCell<AgentTypes>,
    pub agent_instance: RefCell<Option<AgentInstance>>,
    pub agent_initiators: RefCell<AgentInitiators>,
}

#[derive(Default)]
pub struct AgentTypes {
    pub agent_types: HashMap<AgentTypeName, AgentType>,
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

pub type AgentInstance = Arc<RwLock<Box<dyn Agent>>>;

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
        .cloned()
        .collect()
}

pub fn get_agent_type_by_name(agent_type_name: &AgentTypeName) -> Option<AgentType> {
    let state = get_state();

    state
        .agent_types
        .borrow()
        .agent_types
        .get(agent_type_name)
        .cloned()
}

pub fn register_agent_type(agent_type_name: AgentTypeName, agent_type: AgentType) {
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

pub fn register_agent_instance(resolved_agent: Box<dyn Agent>) {
    let state = get_state();

    let agent_instance = Arc::new(RwLock::new(resolved_agent));

    *state.agent_instance.borrow_mut() = Some(agent_instance);
}

// To be used only in agent implementation
pub fn with_agent_instance_async<F, Fut, R>(f: F) -> R
where
    F: FnOnce(AgentInstance) -> Fut,
    Fut: Future<Output = R>,
{
    let state = get_state();

    let agent_instance_optional = state.agent_instance.borrow();

    let agent_instance = agent_instance_optional
        .as_ref()
        .expect("Agent instance not registered")
        .clone();

    block_on(async move { f(agent_instance).await })
}

pub fn is_agent_instance_active() -> bool {
    let state = get_state();

    let agent = state.agent_instance.borrow();

    agent.is_some()
}

pub fn with_agent_instance<F, R>(f: F) -> R
where
    F: FnOnce(AgentInstance) -> R,
{
    let state = get_state();

    let agent = state.agent_instance.borrow();

    let agent = agent
        .as_ref()
        .expect("Agent instance not registered")
        .clone();

    f(agent)
}

pub fn get_agent_id() -> AgentId {
    let env_vars: HashMap<String, String> =
        HashMap::from_iter(crate::bindings::wasi::cli::environment::get_environment());
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

pub fn get_constructor_parameter_type(
    agent_type_name: &AgentTypeName,
    parameter_index: usize,
) -> Option<ElementSchema> {
    let agent_type = get_agent_type_by_name(agent_type_name)?;

    let constructor = &agent_type.constructor;

    match &constructor.input_schema {
        crate::golem_agentic::golem::agent::common::DataSchema::Tuple(items) => {
            if parameter_index < items.len() {
                let element_schema = &items[parameter_index].1;
                Some(element_schema.clone())
            } else {
                None
            }
        }
        crate::golem_agentic::golem::agent::common::DataSchema::Multimodal(items) => {
            if parameter_index < items.len() {
                let element_schema = &items[parameter_index].1;
                Some(element_schema.clone())
            } else {
                None
            }
        }
    }
}

pub fn get_method_parameter_type(
    agent_type_name: &AgentTypeName,
    method_name: &str,
    parameter_index: usize,
) -> Option<ElementSchema> {
    let agent_type = get_agent_type_by_name(agent_type_name)?;

    let method = agent_type.methods.iter().find(|m| m.name == method_name)?;

    match &method.input_schema {
        crate::golem_agentic::golem::agent::common::DataSchema::Tuple(items) => {
            if parameter_index < items.len() {
                let element_schema = &items[parameter_index].1;
                Some(element_schema.clone())
            } else {
                None
            }
        }
        crate::golem_agentic::golem::agent::common::DataSchema::Multimodal(items) => {
            if parameter_index < items.len() {
                let element_schema = &items[parameter_index].1;
                Some(element_schema.clone())
            } else {
                None
            }
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
