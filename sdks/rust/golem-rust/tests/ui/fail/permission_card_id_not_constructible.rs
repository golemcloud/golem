use golem_rust::Uuid;
use golem_rust::permissions::CardId;

fn convert(uuid: Uuid) -> CardId {
    uuid.into()
}

fn read(id: &CardId) -> Uuid {
    id.uuid
}

fn main() {
    let _ = (convert, read);
}
