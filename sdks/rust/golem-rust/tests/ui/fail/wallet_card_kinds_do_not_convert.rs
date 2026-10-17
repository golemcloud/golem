use golem_rust::permissions::{PermissionCard, PolymorphicPermissionCard, ScopeCard, WalletCard};
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

fn template_as_concrete(card: PolymorphicPermissionCard) -> PermissionCard {
    card.into()
}

fn concrete_as_template(card: PermissionCard) -> PolymorphicPermissionCard {
    PolymorphicPermissionCard::from(card)
}

fn scope_as_concrete(card: ScopeCard) -> PermissionCard {
    card.into()
}

fn concrete_as_scope(card: PermissionCard) -> ScopeCard {
    card.into()
}

fn template_in_concrete_case(card: PolymorphicPermissionCard) -> WalletCard {
    WalletCard::Concrete(card)
}

fn wallet_card_as_concrete(card: WalletCard) -> PermissionCard {
    card.into()
}

async fn lend_template_card(card: &PolymorphicPermissionCard) {
    ReceiverClient::get("target".to_string())
        .with_scope_card(card)
        .call()
        .await;
}

fn default_template_card(card: &PolymorphicPermissionCard) {
    let mut client = ReceiverClient::get("target".to_string());
    client.set_default_scope_card(card);
}

fn main() {}
