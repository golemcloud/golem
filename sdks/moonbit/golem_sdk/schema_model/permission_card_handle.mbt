// Opaque, affine carrier for an owned `golem:core/types@2.0.0`
// `permission-card` resource as it travels inside a `SchemaValue`.

///|
/// Opaque, affine (take-once) holder for an owned `permission-card` resource
/// handle. The guest can hold and transfer the card, but cannot inspect or
/// forge the runtime-owned authority represented by the raw resource.
pub(all) struct GuestPermissionCardHandle {
  priv mut cell : @types.PermissionCard?
}

///|
/// Wrap a freshly received owned handle in a take-once holder.
pub fn GuestPermissionCardHandle::new(
  handle : @types.PermissionCard,
) -> GuestPermissionCardHandle {
  { cell: Some(handle) }
}

///|
/// Whether the handle is still present (not yet transferred).
pub fn GuestPermissionCardHandle::is_present(
  self : GuestPermissionCardHandle,
) -> Bool {
  self.cell is Some(_)
}

///|
/// Move the owned handle out, leaving this holder empty.
pub fn GuestPermissionCardHandle::take(
  self : GuestPermissionCardHandle,
) -> @types.PermissionCard? {
  let current = self.cell
  self.cell = None
  current
}

///|
/// Run `f` with the owned handle without consuming it.
pub fn[R] GuestPermissionCardHandle::with_handle(
  self : GuestPermissionCardHandle,
  f : (@types.PermissionCard) -> R,
) -> R? {
  match self.cell {
    Some(h) => Some(f(h))
    None => None
  }
}

///|
/// Identity-based equality: two holders are equal only if they are the same
/// instance. An opaque capability has no structural content to compare.
impl Eq for GuestPermissionCardHandle with fn equal(self, other) {
  physical_equal(self, other)
}

///|
impl Show for GuestPermissionCardHandle with fn output(self, logger) {
  logger.write_string(
    if self.is_present() {
      "GuestPermissionCardHandle(present)"
    } else {
      "GuestPermissionCardHandle(consumed)"
    },
  )
}
