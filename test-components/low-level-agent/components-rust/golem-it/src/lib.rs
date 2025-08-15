#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem::agent::guest::*;
use crate::bindings::golem::agent::common::{AgentConstructor, AgentMethod, DataSchema};
use crate::bindings::golem::agent::host::{register_agent, unregister_agent};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

struct State {
    agents: HashMap<String, WeakTestAgent>,
    last_id: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        agents: HashMap::new(),
        last_id: 0,
    });
}

struct Component;

impl Guest for Component {
    type Agent = TestAgent;

    fn get_agent(_agent_type: String, agent_id: String) -> Agent {
        STATE.with_borrow(|state| {
            if let Some(agent) = state.agents.get(&agent_id) {
                if let Some(agent) = agent.upgrade() {
                    Agent::new(agent)
                } else {
                    panic!("Agent with id {} has been dropped", agent_id);
                }
            } else {
                panic!("Agent with id {} not found", agent_id);
            }
        })
    }

    fn invoke_agent(
        agent_type: String,
        agent_id: String,
        method_name: String,
        input: DataValue,
    ) -> Result<DataValue, AgentError> {
        if agent_type != "TestAgent" {
            Err(AgentError::InvalidType(format!(
                "Invalid agent type: {}",
                agent_type
            )))
        } else {
            STATE.with_borrow(|state| {
                if let Some(agent) = state.agents.get(&agent_id) {
                    if let Some(agent) = agent.upgrade() {
                        agent.invoke(method_name, input)
                    } else {
                        Err(AgentError::InvalidAgentId(format!(
                            "Agent with id {} has been dropped",
                            agent_id
                        )))
                    }
                } else {
                    Err(AgentError::InvalidAgentId(format!(
                        "Agent with id {} not found",
                        agent_id
                    )))
                }
            })
        }
    }

    fn discover_agents() -> Vec<Agent> {
        STATE.with(|state| {
            state
                .borrow()
                .agents
                .values()
                .cloned()
                .filter_map(|agent| agent.upgrade())
                .map(Agent::new)
                .collect()
        })
    }

    fn discover_agent_types() -> Vec<AgentType> {
        vec![agent_type()]
    }
}

#[derive(Clone)]
struct WeakTestAgent {
    state: Weak<RefCell<TestAgentState>>,
}

impl WeakTestAgent {
    pub fn upgrade(&self) -> Option<TestAgent> {
        self.state.upgrade().map(|state| TestAgent { state })
    }
}

#[derive(Clone)]
struct TestAgent {
    state: Rc<RefCell<TestAgentState>>,
}

impl TestAgent {
    pub fn new() -> Self {
        STATE.with_borrow_mut(|state| {
            let agent_id = format!("agent-{}", state.last_id + 1);
            state.last_id += 1;
            let agent = TestAgent {
                state: Rc::new(RefCell::new(TestAgentState {
                    id: agent_id.clone(),
                    inner_agents: vec![],
                })),
            };
            state.agents.insert(agent_id.clone(), agent.downgrade());
            register_agent("TestAgent", &agent_id, &DataValue::Tuple(vec![]));
            agent
        })
    }

    pub fn downgrade(&self) -> WeakTestAgent {
        WeakTestAgent {
            state: Rc::downgrade(&self.state),
        }
    }
}

impl GuestAgent for TestAgent {
    fn create(_agent_type: String, _input: DataValue) -> Result<Agent, AgentError> {
        let agent = TestAgent::new();
        let agent_resource = Agent::new(agent.clone());

        Ok(agent_resource)
    }

    fn get_id(&self) -> String {
        self.state.borrow().id.clone()
    }

    fn invoke(&self, method_name: String, _input: DataValue) -> Result<DataValue, AgentError> {
        match method_name.as_str() {
            "create-inner" => {
                let inner_agent = TestAgent::new();
                self.state.borrow_mut().inner_agents.push(inner_agent);
                Ok(DataValue::Tuple(vec![]))
            }
            "drop-all-inner" => {
                let inner_agents = &mut self.state.borrow_mut().inner_agents;
                for agent in inner_agents.drain(..) {
                    STATE.with_borrow_mut(|state| {
                        state.agents.remove(&agent.state.borrow().id);
                    });
                }
                Ok(DataValue::Tuple(vec![]))
            }
            _ => Err(AgentError::InvalidMethod("Method not found".to_string())),
        }
    }

    fn get_definition(&self) -> AgentType {
        agent_type()
    }
}

fn agent_type() -> AgentType {
    AgentType {
        type_name: "TestAgent".to_string(),
        description: "".to_string(),
        constructor: AgentConstructor {
            name: None,
            description: "".to_string(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(vec![]),
        },
        methods: vec![
            AgentMethod {
                name: "create-inner".to_string(),
                description: "Create a new agent from code".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(vec![]),
                output_schema: DataSchema::Tuple(vec![]),
            },
            AgentMethod {
                name: "drop-all-inner".to_string(),
                description: "Drop all inner agents from code".to_string(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(vec![]),
                output_schema: DataSchema::Tuple(vec![]),
            },
        ],
        dependencies: vec![],
    }
}

struct TestAgentState {
    id: String,
    inner_agents: Vec<TestAgent>,
}

impl Drop for TestAgentState {
    fn drop(&mut self) {
        unregister_agent("TestAgent", &self.id, &DataValue::Tuple(vec![]))
    }
}

bindings::export!(Component with_types_in bindings);
