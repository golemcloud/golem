use golem_rust::agentic::Config;
use golem_rust::bindings::golem::agent::host;
use golem_rust::golem_schema::CardId;
use golem_rust::{ComponentId, ConfigSchema, agent_definition, agent_implementation};
use raw_permissions::golem::permissions::{types, wallet};
use wallet_mutation_tool_guest_client::{
    GolemCoreCardId, GolemCoreComponentId, WalletMutationClient,
};

/// Raw `golem:permissions` calls through this component's own bindings: the
/// tests name wallet cards by id, which the typed `golem_rust::permissions`
/// API never reveals.
mod raw_permissions {
    wit_bindgen::generate!({
        inline: "
            package golem-it:raw-permissions;

            world raw-permissions {
                import golem:permissions/types@0.1.0;
                import golem:permissions/wallet@0.1.0;
            }
        ",
        path: "../../../sdks/rust/golem-rust/wit",
        with: {
            "golem:core/types@2.0.0": golem_rust::schema::wit::wire,
        },
        generate_all,
    });
}

/// The cards of this agent's wallet, of every kind.
fn wallet_cards() -> impl Iterator<Item = golem_rust::schema::wit::wire::PermissionCard> {
    wallet::self_wallet().into_iter().map(|card| match card {
        types::WalletCard::Concrete(card)
        | types::WalletCard::Template(card)
        | types::WalletCard::Scope(card) => card,
    })
}

#[derive(ConfigSchema)]
pub struct WalletRecoveryConfig {
    pub probe: Option<String>,
    pub control: Option<String>,
}

#[agent_definition(snapshotting = "enabled")]
pub trait WalletRecoveryProbe {
    fn new(name: String, #[agent_config] config: Config<WalletRecoveryConfig>) -> Self;
    async fn change(
        &self,
        card: CardId,
        component: ComponentId,
        target: String,
    ) -> (bool, u64, bool, bool);
    fn observe(&self, card: CardId) -> (bool, u64, bool, bool);
}

struct WalletRecoveryProbeImpl;

#[agent_implementation]
impl WalletRecoveryProbe for WalletRecoveryProbeImpl {
    fn new(_name: String, #[agent_config] _config: Config<WalletRecoveryConfig>) -> Self {
        Self
    }

    async fn change(
        &self,
        card: CardId,
        component: ComponentId,
        target: String,
    ) -> (bool, u64, bool, bool) {
        WalletMutationClient::default()
            .change(
                GolemCoreCardId { uuid: card.uuid },
                GolemCoreComponentId {
                    uuid: component.uuid,
                },
                target,
            )
            .await
            .expect("wallet mutation tool completes")
    }

    fn observe(&self, id: CardId) -> (bool, u64, bool, bool) {
        let present = wallet_cards().any(|card| CardId::from(types::id(&card)) == id);
        let generation = wallet::self_version().generation;
        let expected = golem_rust::encode_schema_graph(
            &golem_rust::schema::try_into_schema_graph::<Option<String>>().unwrap(),
        )
        .unwrap();
        let denied = matches!(
            host::get_config_value(&["probe".to_string()], &expected),
            Err(host::ConfigValueError::PermissionDenied)
        );
        let control = host::get_config_value(&["control".to_string()], &expected)
            .map(|bytes| {
                matches!(
                    golem_rust::decode_schema_value(bytes).unwrap(),
                    golem_rust::SchemaValue::Option { inner: None }
                )
            })
            .unwrap_or(false);
        (present, generation, denied, control)
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    async fn load_snapshot(
        _bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Ok(Self)
    }
}
