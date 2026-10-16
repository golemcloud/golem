use golem_rust::bindings::golem::agent::host;
use golem_rust::golem_schema::CardId;
use golem_rust::{ComponentId, tool_definition, tool_implementation};
use raw_permissions::golem::permissions::{revoke, types, wallet};

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
                import golem:permissions/revoke@0.1.0;
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
        let card = wallet_cards()
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
        let present = wallet_cards().any(|card| CardId::from(types::id(&card)) == id);
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
