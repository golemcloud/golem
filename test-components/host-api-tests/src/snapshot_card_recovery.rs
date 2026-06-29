use golem_rust::{
    Card, CardId, Uuid, agent_definition, agent_implementation, derive_card, install_card,
};

#[agent_definition(snapshotting = "enabled")]
pub trait SnapshotCardRecoveryAgent {
    fn new() -> Self;

    fn install_card_by_id(&self, high_bits: u64, low_bits: u64) -> bool;

    fn derive_card_by_id(&self, high_bits: u64, low_bits: u64) -> bool;
}

pub struct SnapshotCardRecoveryAgentImpl;

#[agent_implementation]
impl SnapshotCardRecoveryAgent for SnapshotCardRecoveryAgentImpl {
    fn new() -> Self {
        Self
    }

    fn install_card_by_id(&self, high_bits: u64, low_bits: u64) -> bool {
        install_card(Card {
            card_id: CardId {
                uuid: Uuid::from_u64_pair(high_bits, low_bits),
            },
        })
        .is_ok()
    }

    fn derive_card_by_id(&self, high_bits: u64, low_bits: u64) -> bool {
        derive_card(Card {
            card_id: CardId {
                uuid: Uuid::from_u64_pair(high_bits, low_bits),
            },
        })
        .is_ok()
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    async fn load_snapshot(&mut self, _bytes: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
}
