use golem_rust::permissions::{PermissionCard, PolymorphicPermissionCard};
use golem_rust::schema::wit::GuestPermissionCardHandle;

fn convert(handle: GuestPermissionCardHandle) -> PermissionCard {
    handle.into()
}

fn convert_template(handle: GuestPermissionCardHandle) -> PolymorphicPermissionCard {
    handle.into()
}

fn main() {
    let _ = (convert, convert_template);
    let _ = PermissionCard::new();
    let _ = PolymorphicPermissionCard::new();
}
