use golem_rust::Uuid;
use golem_rust::permissions::{CardId, PermissionCard, PolymorphicPermissionCard};
use golem_rust::schema::wit::GuestPermissionCardHandle;

fn forge_card(handle: GuestPermissionCardHandle) -> PermissionCard {
    PermissionCard { handle }
}

fn forge_template(handle: GuestPermissionCardHandle) -> PolymorphicPermissionCard {
    PolymorphicPermissionCard { handle }
}

fn forge_id(uuid: Uuid) -> CardId {
    CardId { uuid }
}

fn main() {
    let _ = (forge_card, forge_template, forge_id);
}
