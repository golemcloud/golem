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

#[derive(golem_rust::ConfigSchema)]
pub struct WalletSnapshotConfig {
    pub probe: Option<String>,
    pub control: Option<String>,
}

#[agent_definition(snapshotting = "enabled")]
pub trait WalletSnapshotProbe {
    fn new(name: String, #[agent_config] _config: Config<WalletSnapshotConfig>) -> Self;
    fn arm_snapshot_change(
        &self,
        card: golem_rust::CardId,
        component: golem_rust::ComponentId,
        target: String,
    );
    fn observe(&self, card: golem_rust::CardId) -> (bool, u64, bool, bool);
}

pub struct WalletSnapshotProbeImpl {
    change: std::cell::RefCell<Option<(golem_rust::CardId, golem_rust::ComponentId, String)>>,
}

#[agent_implementation]
impl WalletSnapshotProbe for WalletSnapshotProbeImpl {
    fn new(_name: String, #[agent_config] _config: Config<WalletSnapshotConfig>) -> Self {
        Self {
            change: std::cell::RefCell::new(None),
        }
    }

    fn arm_snapshot_change(
        &self,
        card: golem_rust::CardId,
        component: golem_rust::ComponentId,
        target: String,
    ) {
        *self.change.borrow_mut() = Some((card, component, target));
    }

    fn observe(&self, card: golem_rust::CardId) -> (bool, u64, bool, bool) {
        use golem_rust::bindings::golem::permissions::{types, wallet};
        let present = wallet::self_wallet()
            .into_iter()
            .any(|handle| golem_rust::CardId::from(types::id(&handle)) == card);
        let generation = wallet::self_version().generation;
        let expected = golem_rust::encode_schema_graph(
            &golem_rust::schema::try_into_schema_graph::<Option<String>>().unwrap(),
        )
        .unwrap();
        let denied = host::get_config_value(&["probe".to_string()], &expected).is_err();
        let control = host::get_config_value(&["control".to_string()], &expected).is_ok();
        (present, generation, denied, control)
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        use golem_rust::bindings::golem::permissions::{revoke, types, wallet};
        let change = self.change.borrow_mut().take();
        if let Some((id, component, target)) = change {
            let card = wallet::self_wallet()
                .into_iter()
                .find(|handle| golem_rust::CardId::from(types::id(handle)) == id)
                .expect("the save-hook card must be installed");
            if target.is_empty() {
                let count = revoke::revoke_card(card).map_err(|error| format!("{error:?}"))?;
                assert_eq!(count, 1);
            } else {
                wallet::install_card(
                    card,
                    &types::Holder::Agent(golem_rust::schema::wit::wire::AgentId {
                        component_id: component.into(),
                        agent_id: target,
                    }),
                )
                .map_err(|error| format!("{error:?}"))?;
            }
        }
        Ok(Vec::new())
    }

    async fn load_snapshot(
        _bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Ok(Self {
            change: std::cell::RefCell::new(None),
        })
    }
}
