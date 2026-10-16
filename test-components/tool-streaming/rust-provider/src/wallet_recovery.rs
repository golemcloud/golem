use golem_rust::bindings::golem::agent::host;
use golem_rust::bindings::golem::permissions::{revoke, types, wallet};
use golem_rust::{CardId, ComponentId, tool_definition, tool_implementation};

#[tool_definition(version = "1.0.0")]
pub trait WalletMutation {
    async fn change(
        &self,
        card: CardId,
        component: ComponentId,
        target: String,
    ) -> (bool, u64, bool, bool);
}

struct WalletMutationImpl;

#[tool_implementation]
impl WalletMutation for WalletMutationImpl {
    async fn change(
        &self,
        id: CardId,
        component: ComponentId,
        target: String,
    ) -> (bool, u64, bool, bool) {
        let _ = wasi::clocks::wall_clock::now();
        let card = wallet::self_wallet()
            .into_iter()
            .find(|card| CardId::from(types::id(card)) == id)
            .expect("the entity must consume the queued install");
        assert_eq!(wallet::self_version().generation, 1);
        let expected = golem_rust::encode_schema_graph(
            &golem_rust::schema::try_into_schema_graph::<Option<String>>().unwrap(),
        )
        .unwrap();
        assert!(host::get_config_value(&["probe".to_string()], &expected).is_ok());
        if target.is_empty() {
            assert_eq!(revoke::revoke_card(card).expect("revoke installed card"), 1);
        } else {
            wallet::install_card(
                card,
                &types::Holder::Agent(golem_rust::schema::wit::wire::AgentId {
                    component_id: component.into(),
                    agent_id: target,
                }),
            )
            .expect("transfer installed concrete card");
        }
        let present = wallet::self_wallet()
            .into_iter()
            .any(|card| CardId::from(types::id(&card)) == id);
        let generation = wallet::self_version().generation;
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
}
