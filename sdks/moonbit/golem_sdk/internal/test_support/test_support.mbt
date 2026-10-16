// Shared helpers for the SDK's own tests. They run against the fake host in
// `scripts/run-wasm-test.mjs`; production code never imports this package.

///|
/// The `resource-drop-count` kind of secrets.
pub const SECRET_DROPS : Int = 0

///|
/// The `resource-drop-count` kind of quota tokens.
pub const QUOTA_TOKEN_DROPS : Int = 1

///|
/// The `resource-drop-count` kind of permission cards.
pub const PERMISSION_CARD_DROPS : Int = 2

///|
extern "wasm" fn reset_resource_drop_counts_ffi() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn resource_drop_count_ffi(kind : Int) -> Int = "golem:test" "resource-drop-count"

///|
/// Resets the fake host's resource drop counters.
pub fn reset_resource_drop_counts() -> Unit {
  reset_resource_drop_counts_ffi()
}

///|
/// The number of resources of `kind` that the guest dropped since the last
/// reset.
pub fn resource_drop_count(kind : Int) -> Int {
  resource_drop_count_ffi(kind)
}

///|
/// Runs `body` to completion on the guest event loop. Fails if `body` waits on
/// a host event or does not complete.
pub fn run_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps += 1
    if steps > 1000 {
      fail("async test did not complete")
    }
    if (callback & 0xf) != 1 {
      fail("async test unexpectedly waited on a host event")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
/// An invocation input whose encode fails: `card`, a quota token with the
/// handle `quota`, and a secret that was already transferred.
pub fn failing_card_input(
  card : @card.GuestPermissionCardHandle,
  quota : Int,
) -> Array[@model.SchemaValue] {
  let consumed = @model.GuestSecretHandle::new(@types.Secret::Secret(quota + 1))
  ignore(consumed.take())
  [
    PermissionCard(card),
    QuotaToken(
      @model.GuestQuotaTokenHandle::new(@types.QuotaToken::QuotaToken(quota)),
    ),
    Secret(consumed),
  ]
}
