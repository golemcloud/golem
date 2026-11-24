use super::WalletReplayConfig;
use futures::future::{Either, select};
use golem_rust::agentic::Config;
use golem_rust::bindings::golem::agent::host;
use golem_rust::{PromiseId, agent_definition, agent_implementation};

#[agent_definition]
pub trait WalletReplayProbe {
    fn new(name: String, #[agent_config] _config: Config<WalletReplayConfig>) -> Self;
    fn create_release_promise(&self) -> PromiseId;
    async fn cancel_then_read_config(&self, release: PromiseId) -> (String, bool);
}

pub struct WalletReplayProbeImpl;

#[agent_implementation]
impl WalletReplayProbe for WalletReplayProbeImpl {
    fn new(_name: String, #[agent_config] _config: Config<WalletReplayConfig>) -> Self {
        Self
    }

    fn create_release_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    async fn cancel_then_read_config(&self, release: PromiseId) -> (String, bool) {
        {
            let promise = Box::pin(golem_rust::await_promise(&release));
            let clock = Box::pin(golem_rust::wasip3::clocks::monotonic_clock::wait_until(0));
            match select(promise, clock).await {
                Either::Left(_) => panic!("the release promise must remain incomplete"),
                Either::Right(((), promise)) => drop(promise),
            }
        }
        println!("wallet-replay-accessor-dropped");

        let probe = Config::<WalletReplayConfig>::config_entries()
            .into_iter()
            .find(|entry| entry.path == ["probe"])
            .unwrap();
        let probe_expected = golem_rust::encode_schema_graph(&probe.value_type).unwrap();
        let decision = host::get_config_value(&["probe".to_string()], &probe_expected)
            .map(|_| "allowed".to_string())
            .unwrap_or_else(|error| format!("{error:?}"));
        let expected = golem_rust::encode_schema_graph(
            &golem_rust::schema::try_into_schema_graph::<Option<String>>().unwrap(),
        )
        .unwrap();
        let control = host::get_config_value(&["control".to_string()], &expected)
            .map(|value| {
                matches!(
                    golem_rust::decode_schema_value(value).unwrap(),
                    golem_rust::SchemaValue::Option { inner: None }
                )
            })
            .unwrap_or(false);
        (decision, control)
    }
}
