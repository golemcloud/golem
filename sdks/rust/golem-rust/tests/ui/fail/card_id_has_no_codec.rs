use golem_rust::permissions::CardId;
use golem_rust::schema::wit::{direct, wire};

fn lower(id: &CardId) -> golem_rust::SchemaValue {
    golem_rust::IntoSchema::to_value(id)
}

fn decode(tree: wire::SchemaValueTree) -> CardId {
    direct::decode::<CardId>(tree).unwrap()
}

fn main() {
    let _ = (lower, decode);
}
