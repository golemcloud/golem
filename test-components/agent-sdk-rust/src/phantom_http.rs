use golem_rust::agentic::{AgentStream, create_webhook};
use golem_rust::{ForkResult, agent_definition, agent_implementation, endpoint, fork};

#[agent_definition(
    mount = "/selected-query",
    agent_id(query = "instance"),
    create_route = "/selected-query/create",
    webhook_suffix = "/callback"
)]
pub trait SelectedQueryAgent {
    fn new(id: String) -> Self;

    #[endpoint(post = "/set/{value}")]
    fn set(&mut self, value: u32);

    #[endpoint(get = "/state")]
    fn state(&self) -> u32;

    #[endpoint(get = "/query")]
    fn query(&self, value: u32) -> u32;

    #[endpoint(get = "/")]
    fn constructor(&self) -> String;

    #[endpoint(post = "/fork/{value}")]
    fn fork_state(&mut self, value: u32) -> String;

    #[endpoint(put = "/echo")]
    fn echo(&self, input: AgentStream<String>) -> super::durable_streams::EchoOutput;

    #[endpoint(post = "/webhook")]
    async fn webhook(&mut self, callback_server: String) -> u32;
}

struct SelectedQueryAgentImpl {
    state: u32,
    constructor: String,
}

#[agent_implementation]
impl SelectedQueryAgent for SelectedQueryAgentImpl {
    fn new(id: String) -> Self {
        assert!(id != "fail", "private initializer failure");
        Self {
            state: 0,
            constructor: id,
        }
    }

    fn set(&mut self, value: u32) {
        self.state = value;
    }

    fn state(&self) -> u32 {
        self.state
    }

    fn query(&self, value: u32) -> u32 {
        value
    }

    fn constructor(&self) -> String {
        self.constructor.clone()
    }

    fn fork_state(&mut self, value: u32) -> String {
        let details = match fork().expect("self fork is allowed") {
            ForkResult::Original(details) => details,
            ForkResult::Forked(details) => {
                self.state = value;
                details
            }
        };
        let uuid: golem_rust::Uuid = details.forked_agent_id.uuid.into();
        uuid.to_string()
    }

    fn echo(&self, input: AgentStream<String>) -> super::durable_streams::EchoOutput {
        super::durable_streams::EchoOutput {
            output: super::durable_streams::copy_stream(input, |value| value),
        }
    }

    async fn webhook(&mut self, callback_server: String) -> u32 {
        let webhook = create_webhook().expect("webhook creation is allowed");
        super::http::agent::send_json_post(
            &callback_server,
            serde_json::to_vec(&serde_json::json!({ "webhookUrl": webhook.url() })).unwrap(),
        )
        .await
        .unwrap();
        self.state = webhook.await.json().unwrap();
        self.state
    }
}

macro_rules! selector_counter {
    ($agent:ident, $implementation:ident, $mount:tt, $create:tt, $($selector:tt)*) => {
        #[agent_definition(mount = $mount, create_route = $create, agent_id($($selector)*))]
        pub trait $agent {
            fn new(id: String) -> Self;

            #[endpoint(post = "/set/{value}")]
            fn set(&mut self, value: u32);

            #[endpoint(get = "/state")]
            fn state(&self) -> u32;

            #[endpoint(get = "/")]
            fn constructor(&self) -> String;
        }

        struct $implementation { state: u32, constructor: String }

        #[agent_implementation]
        impl $agent for $implementation {
            fn new(id: String) -> Self {
                assert!(id != "fail", "private initializer failure");
                Self { state: 0, constructor: id }
            }
            fn set(&mut self, value: u32) { self.state = value; }
            fn state(&self) -> u32 { self.state }
            fn constructor(&self) -> String { self.constructor.clone() }
        }
    };
}

selector_counter!(
    SelectedRequiredQueryAgent,
    SelectedRequiredQueryAgentImpl,
    "/selected-required-query",
    "/selected-required-query/create",
    query = "instance"
);
selector_counter!(
    SelectedPathAgent,
    SelectedPathAgentImpl,
    "/selected-path/{instance}",
    "/selected-path/create",
    path = "instance"
);
selector_counter!(
    SelectedRequiredPathAgent,
    SelectedRequiredPathAgentImpl,
    "/selected-required-path/{instance}",
    "/selected-required-path/create",
    path = "instance"
);
