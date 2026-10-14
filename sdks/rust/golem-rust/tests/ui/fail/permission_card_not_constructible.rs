use golem_rust::permissions::PermissionCard;
use golem_rust::schema::wit::GuestPermissionCardHandle;

fn convert(handle: GuestPermissionCardHandle) -> PermissionCard {
    handle.into()
}

fn main() {
    let _ = convert;
    let _ = PermissionCard::new();
}
