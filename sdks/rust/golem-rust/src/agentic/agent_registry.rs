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
use crate::agentic::{EnrichedParameterSchema, Principal};
use crate::{
    AgentId, ComponentId, Uuid,
    agentic::{ResolvedAgent, agent_initiator::AgentInitiator},
    golem_agentic::exports::golem::agent::guest::AgentType,
};
use std::rc::Rc;
use std::{cell::RefCell, future::Future};
use std::{collections::HashMap, sync::Arc};
#[cfg(all(test, not(target_arch = "wasm32")))]
use std::{
    pin::Pin,
    sync::{Condvar, Mutex},
    task::{Context, Poll, Waker},
};

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

#[cfg(all(test, not(target_arch = "wasm32")))]
fn block_on_agent_future<F>(future: F) -> F::Output
where
    F: Future,
    F::Output: 'static,
{
    let wake_state = Arc::new(NativeWakeState {
        woken: Mutex::new(false),
        condvar: Condvar::new(),
    });
    let waker = Waker::from(wake_state.clone());
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match Pin::new(&mut future).poll(&mut context) {
            Poll::Ready(result) => break result,
            Poll::Pending => {
                let mut woken = wake_state.woken.lock().unwrap();
                while !*woken {
                    woken = wake_state.condvar.wait(woken).unwrap();
                }
                *woken = false;
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
struct NativeWakeState {
    woken: Mutex<bool>,
    condvar: Condvar,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
impl std::task::Wake for NativeWakeState {
    fn wake(self: Arc<Self>) {
        *self.woken.lock().unwrap() = true;
        self.condvar.notify_one();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        *self.woken.lock().unwrap() = true;
        self.condvar.notify_one();
    }
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

pub fn register_agent_type(agent_type_name: AgentTypeName, mut agent_type: ExtendedAgentType) {
    let mut indices: Vec<usize> = (0..agent_type.methods.len()).collect();
    indices.sort_by(|&a, &b| agent_type.methods[a].name.cmp(&agent_type.methods[b].name));
    agent_type.sorted_method_indices = indices;

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
pub async fn with_agent_instance_async<F, Fut, R>(f: F) -> R
where
    F: FnOnce(Rc<ResolvedAgent>) -> Fut,
    Fut: Future<Output = R> + 'static,
    R: 'static,
{
    let agent_instance = get_state()
        .agent_instance
        .borrow()
        .resolved_agent
        .clone()
        .unwrap();

    f(agent_instance).await
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
        HashMap::from_iter(wasip3::cli::environment::get_environment());
    let raw_agent_id = env_vars
        .get("GOLEM_AGENT_ID")
        .expect("Missing GOLEM_AGENT_ID environment variable"); // This is always provided by the Golem runtime
    let raw_component_id = env_vars
        .get("GOLEM_COMPONENT_ID")
        .expect("Missing GOLEM_COMPONENT_ID environment variable");
    AgentId {
        component_id: ComponentId {
            uuid: Uuid::parse_str(raw_component_id).expect("Invalid GOLEM_COMPONENT_ID"),
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
) -> Option<EnrichedParameterSchema> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;

    extract_parameter_schema(&agent_type.constructor.input_schema, parameter_index)
}

pub fn get_method_parameter_type(
    agent_type_name: &AgentTypeName,
    method_name: &str,
    parameter_index: usize,
) -> Option<EnrichedParameterSchema> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;

    let method = agent_type.methods.iter().find(|m| m.name == method_name)?;

    extract_parameter_schema(&method.input_schema, parameter_index)
}

pub fn get_constructor_parameter_types(
    agent_type_name: &AgentTypeName,
) -> Option<Vec<EnrichedParameterSchema>> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;

    Some(extract_all_parameter_schemas(
        &agent_type.constructor.input_schema,
    ))
}

pub fn get_method_parameter_types(
    agent_type_name: &AgentTypeName,
    method_name: &str,
) -> Option<Vec<EnrichedParameterSchema>> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;
    let method = agent_type.methods.iter().find(|m| m.name == method_name)?;

    Some(extract_all_parameter_schemas(&method.input_schema))
}

pub fn get_method_parameter_types_by_index(
    agent_type_name: &AgentTypeName,
    sorted_method_index: usize,
) -> Option<Vec<EnrichedParameterSchema>> {
    let state = get_state();
    let agent_types = state.agent_types.borrow();
    let agent_type = agent_types.agent_types.get(agent_type_name.0.as_str())?;
    let orig_idx = *agent_type.sorted_method_indices.get(sorted_method_index)?;
    let method = agent_type.methods.get(orig_idx)?;

    Some(extract_all_parameter_schemas(&method.input_schema))
}

fn extract_all_parameter_schemas(
    schema: &[(String, EnrichedParameterSchema)],
) -> Vec<EnrichedParameterSchema> {
    schema.iter().map(|(_, s)| s.clone()).collect()
}

fn extract_parameter_schema(
    schema: &[(String, EnrichedParameterSchema)],
    parameter_index: usize,
) -> Option<EnrichedParameterSchema> {
    schema
        .get(parameter_index)
        .map(|(_, parameter_schema)| parameter_schema.clone())
}

pub async fn with_agent_initiator<F, Fut, R>(f: F, agent_type_name: &AgentTypeName) -> R
where
    F: FnOnce(Arc<dyn AgentInitiator>) -> Fut,
    Fut: Future<Output = R> + 'static,
    R: 'static,
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

    f(agent_initiator).await
}

#[derive(Eq, Hash, PartialEq)]
pub struct AgentTypeName(pub String);

impl std::borrow::Borrow<str> for AgentTypeName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::block_on_agent_future;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::task::{Context, Poll};
    use std::time::Duration;
    use test_r::test;

    struct WakeLater {
        armed: bool,
        ready: Arc<AtomicBool>,
    }

    impl Future for WakeLater {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.ready.load(Ordering::Acquire) {
                return Poll::Ready(());
            }

            if self.armed {
                panic!("future was polled again before its waker fired");
            }

            self.armed = true;
            let ready = self.ready.clone();
            let waker = cx.waker().clone();

            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                ready.store(true, Ordering::Release);
                waker.wake();
            });

            Poll::Pending
        }
    }

    #[test]
    fn native_block_on_waits_for_waker_before_repolling() {
        block_on_agent_future(WakeLater {
            armed: false,
            ready: Arc::new(AtomicBool::new(false)),
        });
    }
}
