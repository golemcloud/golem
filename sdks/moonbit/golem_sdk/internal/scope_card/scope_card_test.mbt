///|
extern "wasm" fn reset_permission_host() = "golem:test" "reset-permission-host"

///|
extern "wasm" fn push_permission_host_buffer(ptr : Int, size : Int) = "golem:test" "push-permission-host-buffer"

///|
extern "wasm" fn guest_malloc(size : Int) -> Int =
  #|(func (param i32) (result i32) (local i32)
  #| local.get 0 i32.const 4 i32.add call $moonbit.gc.malloc
  #| local.tee 1 i32.const 0 call $moonbit.init_array8
  #| local.get 1)

///|
/// Two scope cards of the fake host in `scripts/run-wasm-test.mjs`, which
/// returns the handles 100 and 101 for them.
fn scope_cards() -> Array[@permissions.ScopeCard] raise {
  reset_permission_host()
  for _ in 0..<8 {
    push_permission_host_buffer(guest_malloc(256), 256)
  }
  let wallet = @permissions.wallet()
  [
    @permissions.derive_scope(wallet, @permissions.Bounds::new()),
    @permissions.derive_scope(wallet, @permissions.Bounds::new()),
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
