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

use crate::{
    agentic::{agent_initiator::AgentInitiator, ResolvedAgent},
    golem_agentic::{
        exports::golem::agent::guest::AgentType,
        golem::{agent::common::ElementSchema, api::host::AgentId},
    },
};
use std::{cell::RefCell, future::Future};
use std::{collections::HashMap, sync::Arc};
use wasi_async_runtime::{block_on, Reactor};

#[derive(Default)]
pub struct State {
    pub agent_types: RefCell<AgentTypes>,
    pub agent_instance: RefCell<AgentInstance>,
    pub agent_initiators: RefCell<AgentInitiators>,
    pub async_runtime: RefCell<AsyncRuntime>,
    pub agent_id: RefCell<Option<AgentId>>,
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

#[derive(Default)]
pub struct AgentInstance {
    pub resolved_agent: Option<Arc<ResolvedAgent>>,
}

#[derive(Default)]
pub struct AgentInitiators {
    pub agent_initiators: HashMap<AgentTypeName, Arc<dyn AgentInitiator>>,
}

#[derive(Default)]
pub struct AsyncRuntime {
    pub reactor: Option<Reactor>,
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

pub fn register_agent_instance(resolved_agent: ResolvedAgent) {
    let state = get_state();
    let agent_id = resolved_agent.agent_id.clone();

    state.agent_instance.borrow_mut().resolved_agent = Some(Arc::new(resolved_agent));
    state.agent_id.borrow_mut().replace(agent_id);
}

// To be used only in agent implementation
pub fn with_agent_instance_async<F, Fut, R>(f: F) -> R
where
    F: FnOnce(Arc<ResolvedAgent>) -> Fut,
    Fut: Future<Output = R>,
{
    let agent_instance = get_state()
        .agent_instance
        .borrow()
        .resolved_agent
        .clone()
        .unwrap();

    block_on(|reactor| async move {
        register_reactor(reactor);
        f(agent_instance).await
    })
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

pub fn get_reactor() -> Reactor {
    get_state().async_runtime.borrow().reactor.clone().unwrap()
}

pub fn register_reactor(reactor: Reactor) {
    let state = get_state();

    state.async_runtime.borrow_mut().reactor = Some(reactor);
}

pub fn get_agent_id() -> AgentId {
    get_state().agent_id.borrow().clone().unwrap()
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

// A call to agent initiator is only from outside and should never be happening in any other part of the call
// and hence it is safe to create a reactor and register forever
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
        .unwrap();

    block_on(|reactor| async move {
        register_reactor(reactor);

        f(agent_initiator).await
    })
}

#[derive(Eq, Hash, PartialEq)]
pub struct AgentTypeName(pub String);
