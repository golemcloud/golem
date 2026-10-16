use golem_rust::permissions::{PermissionCard, ScopeCard};
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait Receiver {
    fn new(name: String) -> Self;
    fn call(&self);
}

pub struct ReceiverImpl;

#[agent_implementation]
impl Receiver for ReceiverImpl {
    fn new(_name: String) -> Self {
        Self
    }

    fn call(&self) {}
}

async fn lend_persistent_card(card: &PermissionCard) {
    ReceiverClient::get("target".to_string())
        .with_scope_card(card)
        .call()
        .await;
}

fn default_persistent_card(card: &PermissionCard) {
    let mut client = ReceiverClient::get("target".to_string());
    client.set_default_scope_card(card);
}

fn scope_card_view_has_no_trigger(card: &ScopeCard) {
    ReceiverClient::get("target".to_string())
        .with_scope_card(card)
        .trigger_call();
}

fn scope_card_is_not_a_schema_value(card: &ScopeCard) -> golem_rust::SchemaValue {
    golem_rust::IntoSchema::to_value(card)
}

fn scope_card_cannot_be_revoked(card: ScopeCard) {
    let _ = card.revoke();
}

fn main() {}
