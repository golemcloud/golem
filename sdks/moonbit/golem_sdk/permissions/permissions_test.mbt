// Tests for the typed permission-card API.
//
// The host calls run against the fake `golem:permissions` host in
// `scripts/run-wasm-test.mjs`. Its wallet holds the handles 11 and 12, derived
// cards get fresh handles, `revoke-card` succeeds only for handle 11,
// `install-card` always reports an install recipient mismatch, and
// `derive-from-wallet` rejects a request whose first lower-positive verb is
// `"forbidden"`.

///|
extern "wasm" fn reset_resource_drop_counts() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn resource_drop_count(kind : Int) -> Int = "golem:test" "resource-drop-count"

///|
extern "wasm" fn reset_permission_host() = "golem:test" "reset-permission-host"

///|
extern "wasm" fn push_permission_host_buffer(ptr : Int, size : Int) = "golem:test" "push-permission-host-buffer"

///|
extern "wasm" fn permission_host_stat(field : Int) -> Int = "golem:test" "permission-host-stat"

///|
extern "wasm" fn guest_malloc(size : Int) -> Int =
  #|(func (param i32) (result i32) (local i32)
  #| local.get 0 i32.const 4 i32.add call $moonbit.gc.malloc
  #| local.tee 1 i32.const 0 call $moonbit.init_array8
  #| local.get 1)

///|
const PERMISSION_CARD_DROPS : Int = 2

///|
/// Resets the fake host and gives it guest buffers for returned strings and
/// lists.
fn fresh_host() -> Unit {
  reset_permission_host()
  reset_resource_drop_counts()
  for _ in 0..<32 {
    push_permission_host_buffer(guest_malloc(256), 256)
  }
}

///|
/// Runs `f` and returns the permission error it raises.
fn[T] expect_error(
  f : () -> T raise @permissions.PermissionError,
) -> @permissions.PermissionError raise {
  try f() catch {
    error => error
  } noraise {
    _ => fail("expected a permission error")
  }
}

///|
fn grant(verb : String) -> @permissions.PatternGrant {
  { class: "card", owner: "*", recipient: "*", verb, resource_id: "*", }
}

///|
test "wallet cards compare by id with the parents of a derived card" {
  fresh_host()
  let cards = @permissions.wallet()
  assert_eq(cards.length(), 2)
  let first = cards[0]
  let second = cards[1]
  assert_true(first.id() == first.id())
  assert_true(first.id() != second.id())
  let child = first.derive_child(@permissions.Bounds::new())
  assert_true(child.parents() == [first.id()])
  assert_true(child.parents() != [second.id()])
  let ids : @hashset.HashSet[@permissions.CardId] = @hashset.HashSet([])
  ids.add(first.id())
  ids.add(first.id())
  ids.add(second.id())
  assert_eq(ids.length(), 2)
  // The identifier is redacted in every textual form.
  debug_inspect(first.id(), content="CardId(<redacted>)")
  inspect(first.id(), content="CardId(<redacted>)")
  assert_eq("\{first.id()}", "CardId(<redacted>)")
  assert_eq(first.id().to_json().stringify(), "\"CardId(<redacted>)\"")
}

///|
test "derive_child borrows the parent and sends the lifespan" {
  fresh_host()
  let parent = @permissions.wallet()[1]
  let child = parent.derive_child(
    @permissions.Bounds::new(upper_positive=[grant("inspect")]),
    lifespan=@permissions.Lifespan::iso8601("PT1H"),
  )
  assert_eq(permission_host_stat(4), 12)
  assert_eq(permission_host_stat(6), 4)
  debug_inspect(
    child.expires_at(),
    content="Some({ seconds: 1700000000, nanoseconds: 5 })",
  )
  // The parent is kept and still answers.
  assert_true(child.parents() == [parent.id()])
  let indefinite = parent.derive_child(@permissions.Bounds::new())
  assert_eq(permission_host_stat(6), -1)
  debug_inspect(indefinite.expires_at(), content="None")
}

///|
test "revoke returns the host count and moves the card" {
  fresh_host()
  let card = @permissions.wallet()[0]
  assert_eq(card.revoke(), 3U)
  assert_eq(permission_host_stat(0), 1)
  assert_eq(permission_host_stat(1), 11)
  // The host owns the handle now; dropping the moved card releases nothing.
  card.drop()
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 0)
}

///|
test "a failed revoke still moves the card" {
  fresh_host()
  let card = @permissions.wallet()[1]
  let error = expect_error(() => card.revoke())
  guard error is CardRevoked(message) else {
    fail("expected card-revoked, got \{repr(error)}")
  }
  assert_eq(message, "permission card is unknown or revoked")
  card.drop()
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 0)
  assert_eq(permission_host_stat(0), 1)
}

///|
test "install maps a recipient mismatch and moves the card" {
  fresh_host()
  let card = @permissions.wallet()[1]
  let target : @permissions.Holder = Agent({
    component_id: { uuid: { high_bits: 1, low_bits: 2, }, },
    agent_id: "Target(\"other\")",
  })
  let error = expect_error(() => card.install(target))
  assert_eq(error.kind(), "install-recipient-mismatch")
  assert_eq(error.message(), "card does not cover the recipient")
  inspect(
    error,
    content="install-recipient-mismatch: card does not cover the recipient",
  )
  assert_eq(permission_host_stat(3), 12)
  card.drop()
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 0)
}

///|
test "derive_from_wallet maps not-permitted" {
  fresh_host()
  let denied = expect_error(() => {
    @permissions.derive_from_wallet(
      @permissions.Bounds::new(lower_positive=[grant("forbidden")]),
    )
  })
  guard denied is NotPermitted(message) else {
    fail("expected not-permitted, got \{repr(denied)}")
  }
  assert_eq(message, "card:derive is not permitted")
  let card = @permissions.derive_from_wallet(
    @permissions.Bounds::new(lower_positive=[grant("inspect")]),
    lifespan=@permissions.Lifespan::from_duration(
      @schema.Duration::new(90_000_000_000L),
    ),
  )
  assert_eq(permission_host_stat(6), "PT1M30S".length())
  assert_true(card.parents() == [@permissions.wallet()[0].id()])
}

///|
test "derive_scope borrows every parent" {
  fresh_host()
  let wallet = @permissions.wallet()
  let scope = @permissions.derive_scope(
    wallet,
    @permissions.Bounds::new(lower_positive=[grant("inspect")]),
  )
  assert_eq(permission_host_stat(5), 2)
  assert_true(scope.parents() == [wallet[0].id(), wallet[1].id()])
  @permissions.drop_all(wallet)
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 2)
  assert_false(scope.is_moved())
}

///|
test "a scope card can be a parent of a scope card" {
  fresh_host()
  let wallet = @permissions.wallet()
  let outer = @permissions.derive_scope([wallet[0]], @permissions.Bounds::new())
  let inner = @permissions.derive_scope(
    [wallet[1]],
    @permissions.Bounds::new(),
    scope_parents=[outer],
  )
  assert_eq(permission_host_stat(5), 2)
  // `parents` names root card ids, sorted and without duplicates: `outer`
  // contributes its root.
  assert_true(inner.parents() == [wallet[0].id(), wallet[1].id()])
  let repeated = @permissions.derive_scope(
    [wallet[0]],
    @permissions.Bounds::new(),
    scope_parents=[outer, inner],
  )
  assert_eq(permission_host_stat(5), 3)
  assert_true(repeated.parents() == [wallet[0].id(), wallet[1].id()])
  let scope_only = @permissions.derive_scope([], @permissions.Bounds::new(), scope_parents=[
    outer,
  ])
  assert_eq(permission_host_stat(5), 1)
  assert_true(scope_only.parents() == [wallet[0].id()])
  debug_inspect(outer.expires_at(), content="None")
}

///|
test "drop releases a scope card once" {
  fresh_host()
  let wallet = @permissions.wallet()
  let scope = @permissions.derive_scope(wallet, @permissions.Bounds::new())
  scope.drop()
  scope.drop()
  assert_true(scope.is_moved())
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 1)
  assert_false(wallet[0].is_moved())
}

///|
test "panic using a dropped scope card aborts" {
  fresh_host()
  let scope = @permissions.derive_scope(
    @permissions.wallet(),
    @permissions.Bounds::new(),
  )
  scope.drop()
  ignore(scope.id())
}

///|
test "inspect lifts the card view" {
  fresh_host()
  let wallet = @permissions.wallet()
  debug_inspect(
    wallet[0].inspect(),
    content=(
      #|{
      #|  lower_positive: [
      #|    {
      #|      class: "card",
      #|      owner: "alice/*",
      #|      recipient: "*",
      #|      verb: "inspect",
      #|      resource_id: "*",
      #|    },
      #|  ],
      #|  lower_negative: [],
      #|  upper_positive: [],
      #|  upper_negative: [],
      #|}
    ),
  )
  let denied = expect_error(() => wallet[1].inspect())
  guard denied is NotPermitted(_) else {
    fail("expected not-permitted, got \{repr(denied)}")
  }
}

///|
test "kernel introspection and wallet version lift host data" {
  fresh_host()
  debug_inspect(
    @permissions.list_modules(),
    content=(
      #|[{ class_name: "card", verbs: ["derive", "inspect"] }]
    ),
  )
  @permissions.validate_grant(grant("read"))
  let invalid = expect_error(() => @permissions.validate_grant(grant("fly")))
  guard invalid is InvalidVerb("fly") else {
    fail("expected invalid-verb, got \{repr(invalid)}")
  }
  let version = @permissions.wallet_version()
  assert_eq(version.generation, 9UL)
  assert_eq(version.wallet_id_hash.length(), 32)
  assert_eq(version.wallet_id_hash[31], b'\x1f')
}

///|
test "drop releases a held card once" {
  fresh_host()
  let card = @permissions.wallet()[0]
  card.drop()
  card.drop()
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 1)
}

///|
test "panic using a revoked card aborts" {
  fresh_host()
  let card = @permissions.wallet()[0]
  ignore(card.revoke())
  ignore(card.id())
}

///|
test "panic revoking an installed card aborts" {
  fresh_host()
  let card = @permissions.wallet()[1]
  card.install(App({ high_bits: 0, low_bits: 0, })) catch {
    _ => ()
  }
  ignore(card.revoke()) catch {
    _ => ()
  }
}

///|
test "panic deriving a scope from a moved parent aborts" {
  fresh_host()
  let wallet = @permissions.wallet()
  wallet[0].drop()
  ignore(@permissions.derive_scope(wallet, @permissions.Bounds::new())) catch {
    _ => ()
  }
}

///|
test "a card lowered into an agent value moves exactly once" {
  fresh_host()
  let card = @permissions.wallet()[0]
  let value = @schema.to_value_as(card)
  guard value is PermissionCard(_) else {
    fail("expected a permission-card schema value, got \{repr(value)}")
  }
  // Encoding the value for the host takes the owned handle out of the holder
  // that the card shares.
  let tree = @model.schema_value_to_wit(value)
  debug_inspect(
    tree.value_nodes,
    content="[PermissionCardHandle(PermissionCard(11))]",
  )
  assert_true(card.is_moved())
  card.drop()
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 0)
  // The host owns the handle, so a received tree with it decodes into a card.
  let received : @permissions.PermissionCard = @schema.from_wire(tree)
  assert_false(received.is_moved())
}

///|
/// A wire tree that names the handle of a held card, built with the public raw
/// bindings.
fn forged_tree(index : Int) -> @types.SchemaValueTree {
  { value_nodes: [PermissionCardHandle(PermissionCard(index))], root: 0, }
}

///|
test "decoding never aliases a held permission or scope card" {
  fresh_host()
  let wallet = @permissions.wallet()
  let scope = @permissions.derive_scope(wallet, @permissions.Bounds::new())
  let wire_alias : @permissions.PermissionCard? = Some(
    @schema.from_wire(forged_tree(11)),
  ) catch {
    _ => None
  }
  assert_true(wire_alias is None)
  let value_alias = @model_host.schema_value_from_wit(forged_tree(12)) catch {
    _ => @model.SchemaValue::Bool(false)
  }
  debug_inspect(value_alias, content="Bool(false)")
  // The scope card's handle is 100; it cannot become a persistent card.
  let scope_alias : @permissions.PermissionCard? = Some(
    @schema.from_wire(forged_tree(100)),
  ) catch {
    _ => None
  }
  assert_true(scope_alias is None)
  // The rejected decodes neither released nor moved the held cards.
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 0)
  assert_false(wallet[0].is_moved())
  assert_false(wallet[1].is_moved())
  assert_false(scope.is_moved())
  // A schema value of a card shares the card's take-once holder, so the
  // decoded card is the same card.
  let shared : @permissions.PermissionCard = @schema.from_value_as(
    @schema.to_value_as(wallet[0]),
  )
  shared.drop()
  assert_true(wallet[0].is_moved())
  assert_eq(resource_drop_count(PERMISSION_CARD_DROPS), 1)
}

///|
test "decoding a handle that no card holds yields a card" {
  fresh_host()
  // The SDK cannot tell such a handle from one the host sent. The host checks
  // it on every use and traps on an unknown handle.
  let card : @permissions.PermissionCard = @schema.from_wire(forged_tree(77))
  assert_false(card.is_moved())
  let second : @permissions.PermissionCard? = Some(
    @schema.from_wire(forged_tree(77)),
  ) catch {
    _ => None
  }
  assert_true(second is None)
}

///|
test "permission card schema is a concrete capability node" {
  let graph = @schema.into_schema_graph(
    (@schema.type_tag() : @schema.TypeTag[@permissions.PermissionCard]),
  )
  inspect(graph.defs.length(), content="0")
  debug_inspect(
    graph.root.body,
    content="PermissionCard({ polymorphic: false })",
  )
  let decoded : @permissions.PermissionCard? = Some(
    @schema.from_value_as(@model.SchemaValue::String("card-id")),
  ) catch {
    _ => None
  }
  guard decoded is None else {
    fail("a string must not decode into a permission card")
  }
}

///|
test "permission error has one case per WIT case" {
  let errors : Array[@permissions.PermissionError] = [
    NotPermitted("m"),
    LowerBoundTooBroad("m"),
    UpperBoundTooBroad("m"),
    WalletMultiSourceRequired("m"),
    CardRevoked("m"),
    CardExpired("m"),
    InstallRecipientMismatch("m"),
    UnknownResourceClass("m"),
    InvalidPattern("m"),
    InvalidVerb("m"),
    InvalidOwner("m"),
    InvalidRecipient("m"),
  ]
  let kinds = errors.map(fn(error) { error.kind() })
  // The WIT `permission-error` variant has 12 cases.
  assert_eq(kinds.length(), 12)
  let distinct : @hashset.HashSet[String] = @hashset.from_array(kinds)
  assert_eq(distinct.length(), 12)
  for error in errors {
    assert_eq(error.message(), "m")
  }
}

///|
test "lifespan renders durations in the PnDTnHnMnS form" {
  let render = fn(nanoseconds : Int64) raise {
    @permissions.Lifespan::from_duration(@schema.Duration::new(nanoseconds)).to_iso8601()
  }
  assert_eq(render(1L), "PT0.000000001S")
  assert_eq(render(3_600_000_000_000L), "PT1H")
  assert_eq(render(93_784_500_000_000L), "P1DT2H3M4.5S")
  assert_eq(render(86_400_000_000_000L), "P1D")
  assert_eq(render(9_223_372_036_854_775_807L), "P106751DT23H47M16.854775807S")
  assert_eq(@permissions.Lifespan::iso8601("P1DT2H").to_iso8601(), "P1DT2H")
}

///|
test "lifespan rejects zero and negative durations" {
  for nanoseconds in [0L, -1L, -9_223_372_036_854_775_808L] {
    let error = expect_error(() => {
      @permissions.Lifespan::from_duration(@schema.Duration::new(nanoseconds))
    })
    assert_eq(error.kind(), "invalid-pattern")
  }
}
