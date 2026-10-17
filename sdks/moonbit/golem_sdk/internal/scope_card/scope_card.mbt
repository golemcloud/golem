// Access to the raw handle of a scope card for the SDK's RPC clients. The
// package is internal to the `golemcloud/golem_sdk` module, so user code cannot
// borrow the raw handle of a `ScopeCard` through it.

///|
/// The raw handle of `card`, borrowed for one host call. The card is kept.
/// Aborts if the card was dropped.
pub fn borrow_raw(card : @permissions.ScopeCard) -> @types.PermissionCard {
  match @card.borrow_scope(card.handle) {
    Some(raw) => raw
    None =>
      abort(
        "the scope card has already been dropped and cannot be attached to an RPC call",
      )
  }
}

///|
/// The raw scope-card argument of an awaited RPC call: the per-call card when
/// present, otherwise the client default. Resolve it before encoding the call
/// input, so that a dropped scope card aborts before any argument card moves.
pub fn resolve(
  per_call : @permissions.ScopeCard?,
  default : @permissions.ScopeCard?,
) -> @types.PermissionCard? {
  match per_call {
    Some(card) => Some(borrow_raw(card))
    None => default.map(borrow_raw)
  }
}
