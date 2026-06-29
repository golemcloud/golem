// Opaque, affine carrier for an owned `golem:core/types@2.0.0` `quota-token`
// resource as it travels inside a `SchemaValue`.

///|
/// Opaque, affine (take-once) holder for an owned `golem:core/types@2.0.0`
/// `quota-token` resource handle.
///
/// A `quota-token` is an affine capability: the guest may hold and transfer it,
/// but can never inspect or forge it. The raw owned resource is hidden behind
/// this holder (the `cell` field is package-private) so the supported user-facing
/// `@quota.QuotaToken` wrapper can only move the handle to a single destination —
/// an RPC argument, a method return value, … Lowering a `SchemaValue` that
/// carries a token takes the handle out exactly once (first encode wins);
/// decoding wraps a freshly received owned handle in a new holder.
///
/// `new` / `take` / `with_handle` are SDK-internal plumbing shared between this
/// package and `@quota`: MoonBit has no cross-package-but-not-public visibility,
/// so they are `pub` only because both packages need them. They are NOT a
/// supported user API — going through them to extract or re-wrap a raw handle is
/// unsupported. As defence in depth the codec still rejects the same underlying
/// owned resource appearing twice in one value tree (by resource identity, not
/// only holder identity), so a manually re-wrapped handle cannot bypass the
/// affine encode/decode checks.
///
/// Equality is by identity so the same handle appearing twice in one value tree
/// can be detected during encoding; an opaque capability has no structural
/// content to compare.
pub(all) struct GuestQuotaTokenHandle {
  priv mut cell : @types.QuotaToken?
}

///|
/// Wrap a freshly received owned handle in a take-once holder.
pub fn GuestQuotaTokenHandle::new(
  handle : @types.QuotaToken,
) -> GuestQuotaTokenHandle {
  { cell: Some(handle) }
}

///|
/// Whether the handle is still present (not yet transferred).
pub fn GuestQuotaTokenHandle::is_present(self : GuestQuotaTokenHandle) -> Bool {
  self.cell is Some(_)
}

///|
/// Move the owned handle out, leaving this holder empty. Returns `None` if the
/// handle was already transferred.
pub fn GuestQuotaTokenHandle::take(
  self : GuestQuotaTokenHandle,
) -> @types.QuotaToken? {
  let current = self.cell
  self.cell = None
  current
}

///|
/// Run `f` with the owned handle without consuming it (for borrowing host
/// operations such as `reserve` / `split`). Returns `None` if the handle was
/// already transferred.
pub fn[R] GuestQuotaTokenHandle::with_handle(
  self : GuestQuotaTokenHandle,
  f : (@types.QuotaToken) -> R,
) -> R? {
  match self.cell {
    Some(h) => Some(f(h))
    None => None
  }
}

///|
/// Identity-based equality: two holders are equal only if they are the same
/// instance. A `SchemaValue` carrying a token therefore compares by handle
/// identity rather than by any (non-existent) structural content.
impl Eq for GuestQuotaTokenHandle with fn equal(self, other) {
  physical_equal(self, other)
}

///|
impl Show for GuestQuotaTokenHandle with fn output(self, logger) {
  logger.write_string(
    if self.is_present() {
      "GuestQuotaTokenHandle(present)"
    } else {
      "GuestQuotaTokenHandle(consumed)"
    },
  )
}
