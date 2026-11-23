///|
extern "wasm" fn reset_permission_host() = "golem:test" "reset-permission-host"

///|
extern "wasm" fn push_permission_host_buffer(ptr : Int32, size : Int32) = "golem:test" "push-permission-host-buffer"

///|
fn guest_malloc(size : Int32) -> Int32 {
  @gen.mbt_ffi_cabi_realloc(0, 0, 4, size)
}

///|
/// Two scope cards of the fake host in `scripts/run-wasm-test.mjs`, which
/// returns the handles 100 and 101 for them.
fn scope_cards() -> Array[@permissions.ScopeCard] raise {
  reset_permission_host()
  for _ in 0..<8 {
    push_permission_host_buffer(guest_malloc(256), 256)
  }
  let parents = []
  for card in @permissions.wallet() {
    match card {
      Concrete(card) => parents.push(card)
      other => other.drop()
    }
  }
  [
    @permissions.derive_scope(parents, @permissions.Bounds::new()),
    @permissions.derive_scope(parents, @permissions.Bounds::new()),
  ]
}

///|
test "the per-call scope card overrides the client default" {
  let cards = scope_cards()
  debug_inspect(
    @scope_card.resolve(Some(cards[1]), Some(cards[0])),
    content="Some(PermissionCard(101))",
  )
  debug_inspect(
    @scope_card.resolve(None, Some(cards[0])),
    content="Some(PermissionCard(100))",
  )
  debug_inspect(@scope_card.resolve(None, None), content="None")
  // Attaching borrows: the cards are still held.
  debug_inspect(
    @scope_card.resolve(Some(cards[1]), None),
    content="Some(PermissionCard(101))",
  )
}

///|
test "panic attaching a dropped scope card aborts" {
  let cards = scope_cards()
  cards[0].drop()
  ignore(@scope_card.resolve(None, Some(cards[0])))
}
