use golem_rust::agentic::{AgentStream, create_webhook};
use golem_rust::{ForkResult, agent_definition, agent_implementation, endpoint, fork};

#[agent_definition(
    mount = "/selected-query/{id}",
    phantom_agent = true,
    phantom_id(query = "instance", optional = true),
    webhook_suffix = "/callback"
)]
pub trait SelectedQueryAgent {
    fn new(id: String) -> Self;

    #[endpoint(post = "/set/{value}")]
    fn set(&mut self, value: u32);

    #[endpoint(get = "/state")]
    fn state(&self) -> u32;

    #[endpoint(post = "/fork/{value}")]
    fn fork_state(&mut self, value: u32) -> String;

    #[endpoint(put = "/echo")]
    fn echo(&self, input: AgentStream<String>) -> super::durable_streams::EchoOutput;

    #[endpoint(post = "/webhook")]
    async fn webhook(&mut self, callback_server: String) -> u32;
}

struct SelectedQueryAgentImpl {
    state: u32,
}

#[agent_implementation]
impl SelectedQueryAgent for SelectedQueryAgentImpl {
    fn new(_id: String) -> Self {
        Self { state: 0 }
    }

    fn set(&mut self, value: u32) {
        self.state = value;
    }

    fn state(&self) -> u32 {
        self.state
    }

    fn fork_state(&mut self, value: u32) -> String {
        let details = match fork().expect("self fork is allowed") {
            ForkResult::Original(details) => details,
            ForkResult::Forked(details) => {
                self.state = value;
                details
            }
        };
        let uuid: golem_rust::Uuid = details.forked_phantom_id.into();
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
    ($agent:ident, $implementation:ident, $mount:tt, $($selector:tt)*) => {
        #[agent_definition(mount = $mount, phantom_id($($selector)*))]
        pub trait $agent {
            fn new(id: String) -> Self;

            #[endpoint(post = "/set/{value}")]
            fn set(&mut self, value: u32);

            #[endpoint(get = "/state")]
            fn state(&self) -> u32;
        }

        struct $implementation { state: u32 }

        #[agent_implementation]
        impl $agent for $implementation {
            fn new(_id: String) -> Self { Self { state: 0 } }
            fn set(&mut self, value: u32) { self.state = value; }
            fn state(&self) -> u32 { self.state }
        }
    };
}

selector_counter!(
    SelectedRequiredQueryAgent,
    SelectedRequiredQueryAgentImpl,
    "/selected-required-query/{id}",
    query = "instance"
);
selector_counter!(
    SelectedPathAgent,
    SelectedPathAgentImpl,
    "/selected-path/{instance}/{id}",
    path = "instance",
    optional = true
);
selector_counter!(
    SelectedRequiredPathAgent,
    SelectedRequiredPathAgentImpl,
    "/selected-required-path/{instance}/{id}",
    path = "instance"
);
