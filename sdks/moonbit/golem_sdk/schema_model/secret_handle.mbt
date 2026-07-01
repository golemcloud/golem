// Opaque, affine carrier for an owned `golem:core/types@2.0.0` `secret`
// resource as it travels inside a `SchemaValue`.

///|
/// Opaque, affine (take-once) holder for an owned `golem:core/types@2.0.0`
/// `secret` resource handle.
///
/// A `secret` is an opaque capability: the guest may hold, transfer, and reveal
/// it through imported host interfaces, but can never inspect or forge it.
/// Lowering a `SchemaValue` that carries a secret takes the handle out exactly
/// once; decoding wraps a freshly received owned handle in a new holder.
pub(all) struct GuestSecretHandle {
  priv mut cell : @types.Secret?
}

///|
/// Wrap a freshly received owned handle in a take-once holder.
pub fn GuestSecretHandle::new(handle : @types.Secret) -> GuestSecretHandle {
  { cell: Some(handle) }
}

///|
/// Whether the handle is still present (not yet transferred).
pub fn GuestSecretHandle::is_present(self : GuestSecretHandle) -> Bool {
  self.cell is Some(_)
}

///|
/// Move the owned handle out, leaving this holder empty. Returns `None` if the
/// handle was already transferred.
pub fn GuestSecretHandle::take(self : GuestSecretHandle) -> @types.Secret? {
  let current = self.cell
  self.cell = None
  current
}

///|
/// Run `f` with the owned handle without consuming it (for borrowing host
/// operations such as `reveal`). Returns `None` if the handle was already
/// transferred.
pub fn[R] GuestSecretHandle::with_handle(
  self : GuestSecretHandle,
  f : (@types.Secret) -> R,
) -> R? {
  match self.cell {
    Some(h) => Some(f(h))
    None => None
  }
}

///|
/// Identity-based equality: two holders are equal only if they are the same
/// instance. An opaque capability has no structural content to compare.
impl Eq for GuestSecretHandle with fn equal(self, other) {
  physical_equal(self, other)
}

///|
impl Show for GuestSecretHandle with fn output(self, logger) {
  logger.write_string(
    if self.is_present() {
      "GuestSecretHandle(present)"
    } else {
      "GuestSecretHandle(consumed)"
    },
  )
}
