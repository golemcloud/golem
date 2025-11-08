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
    pub inner_state: RefCell<InnerState>,
}

#[derive(Default)]
pub struct InnerState {
    pub agent_types: HashMap<AgentTypeName, AgentType>,
    pub agent_instance: Option<Arc<ResolvedAgent>>,
    pub agent_initiators: HashMap<AgentTypeName, Arc<dyn AgentInitiator>>,
    pub reactor: Option<Reactor>,
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

pub fn get_all_agent_types() -> Vec<AgentType> {
    let state = get_state();

    state
        .inner_state
        .borrow()
        .agent_types
        .values()
        .cloned()
        .collect()
}

pub fn get_agent_type_by_name(agent_type_name: &AgentTypeName) -> Option<AgentType> {
    let state = get_state();

    state
        .inner_state
        .borrow()
        .agent_types
        .get(agent_type_name)
        .cloned()
}

pub fn register_agent_type(agent_type_name: AgentTypeName, agent_type: AgentType) {
    get_state()
        .inner_state
        .borrow_mut()
        .agent_types
        .insert(agent_type_name, agent_type);
}

pub fn register_agent_initiator(agent_type_name: &str, initiator: Arc<dyn AgentInitiator>) {
    let state = get_state();
    let agent_type_name = AgentTypeName(agent_type_name.to_string());

    state
        .inner_state
        .borrow_mut()
        .agent_initiators
        .insert(agent_type_name, initiator);
}

pub fn register_agent_instance(resolved_agent: ResolvedAgent) {
    let state = get_state();

    state.inner_state.borrow_mut().agent_instance = Some(Arc::new(resolved_agent));
}

// To be used only in agent implementation
pub fn with_agent_instance_async<F, Fut, R>(f: F) -> R
where
    F: FnOnce(Arc<ResolvedAgent>) -> Fut,
    Fut: Future<Output = R>,
{
    let agent_instance = {
        let state = get_state().inner_state.borrow();
        state.agent_instance.as_ref().unwrap().clone()
    };

    block_on(|reactor| async move {
        register_reactor(reactor);
        f(agent_instance).await
    })
}

pub fn with_agent_instance<F, R>(f: F) -> R
where
    F: FnOnce(&ResolvedAgent) -> R,
{
    let state = get_state().inner_state.borrow();
    let agent_instance = state.agent_instance.as_ref();

    f(agent_instance.as_ref().unwrap())
}

pub fn get_reactor() -> Reactor {
    get_state().inner_state.borrow().reactor.clone().unwrap()
}

pub fn register_reactor(reactor: Reactor) {
    let state = get_state();

    state.inner_state.borrow_mut().reactor = Some(reactor);
}

pub fn get_agent_id() -> AgentId {
    with_agent_instance(|resolved_agent| resolved_agent.agent_id.clone())
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

    let inner_borrow = state.inner_state.borrow();

    let initiator = inner_borrow
        .agent_initiators
        .get(agent_type_name)
        .unwrap()
        .clone();

    block_on(|reactor| async move {
        register_reactor(reactor);

        f(initiator).await
    })
}

#[derive(Eq, Hash, PartialEq)]
pub struct AgentTypeName(pub String);
