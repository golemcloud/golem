// Permission-card scenario for the MoonBit SDK: wallet derivation, inspection,
// child derivation with a lifespan, scope cards on awaited RPC calls, install
// on another agent, revocation, and the negative cases.

///|
/// Exercises the typed `@permissions` API against the worker executor.
#derive.agent
struct MoonBitPermissionAgent {
  name : String
}

///|
fn MoonBitPermissionAgent::new(name : String) -> MoonBitPermissionAgent {
  { name, }
}

///|
fn is_template_grant(grant : @permissions.PatternGrant) -> Bool {
  grant.owner.contains("?") || grant.resource_id.contains("?")
}

///|
fn is_inspect_grant(grant : @permissions.PatternGrant) -> Bool {
  grant.class == "card" && grant.verb == "inspect" && grant.resource_id == "*"
}

///|
/// The agent type's initial template card from the wallet, if the wallet still
/// holds it. The other wallet cards are released.
fn template_root() -> @permissions.PermissionCard? {
  let mut root : @permissions.PermissionCard? = None
  for card in @permissions.wallet() {
    let is_template = card.inspect().lower_positive
      .iter()
      .any(is_template_grant) catch {
        _ => false
      }
    if root is None && is_template {
      root = Some(card)
    } else {
      card.drop()
    }
  }
  root
}

///|
fn require_template_root() -> @permissions.PermissionCard {
  match template_root() {
    Some(card) => card
    None => abort("the wallet has no template card")
  }
}

///|
fn inspect_grant(
  card : @permissions.PermissionCard,
) -> @permissions.PatternGrant raise @permissions.PermissionError {
  match card.inspect().lower_positive.iter().find_first(is_inspect_grant) {
    Some(grant) => grant
    None => abort("the card does not grant card inspection")
  }
}

///|
fn inspect_bounds(grant : @permissions.PatternGrant) -> @permissions.Bounds {
  @permissions.Bounds::new(lower_positive=[grant])
}

///|
fn self_holder(agent_id : String) -> @permissions.Holder {
  Agent({
    component_id: @api.get_self_metadata().agent_id.component_id,
    agent_id,
  })
}

///|
/// Steps 1 to 3: derives a card from the wallet, inspects it, and derives
/// children with a lifespan from the derived card and from the template root.
/// Returns one `name=value` observation per check.
pub fn MoonBitPermissionAgent::derive_chain(
  self : Self,
) -> Result[Array[String], String] {
  ignore(self)
  let observations = []
  let root = require_template_root()
  defer root.drop()
  try {
    let grant = inspect_grant(root)
    let card = @permissions.derive_from_wallet(inspect_bounds(grant))
    defer card.drop()
    let view = card.inspect()
    observations.push(
      "wallet-card-inspect-grants=\{view.lower_positive.filter(is_inspect_grant).length()}",
    )
    observations.push(
      "wallet-card-other-grants=\{view.lower_negative.length() + view.upper_positive.length() + view.upper_negative.length()}",
    )
    observations.push("wallet-card-expires=\{card.expires_at() is Some(_)}")
    let child = card.derive_child(
      @permissions.Bounds::new(lower_positive=view.lower_positive),
      lifespan=@permissions.Lifespan::iso8601("PT1H"),
    )
    defer child.drop()
    observations.push("child-parent-is-card=\{child.parents() == [card.id()]}")
    observations.push("child-id-differs=\{child.id() != card.id()}")
    observations.push("child-expires=\{child.expires_at() is Some(_)}")
    observations.push(
      "child-inspect-grants=\{child.inspect().lower_positive.filter(is_inspect_grant).length()}",
    )
    let root_child = root.derive_child(
      inspect_bounds(grant),
      lifespan=@permissions.Lifespan::from_duration(
        @schema.Duration::new(60_000_000_000L),
      ),
    )
    defer root_child.drop()
    observations.push(
      "root-child-parent-is-root=\{root_child.parents() == [root.id()]}",
    )
    observations.push(
      "root-child-expires=\{root_child.expires_at() is Some(_)}",
    )
    try {
      card
      .derive_child(
        inspect_bounds(grant),
        lifespan=@permissions.Lifespan::iso8601("P1M"),
      )
      .drop()
      observations.push("calendar-lifespan=accepted")
    } catch {
      error => observations.push("calendar-lifespan=\{error.kind()}")
    }
    try {
      ignore(@permissions.Lifespan::from_duration(@schema.Duration::new(0L)))
      observations.push("zero-lifespan=accepted")
    } catch {
      error => observations.push("zero-lifespan=\{error.kind()}")
    }
    Ok(observations)
  } catch {
    error => Err(error.to_string())
  }
}

///|
/// Step 7: an install whose card does not cover the recipient, an account
/// target, and an invalid grant. Returns the error kind of each case.
pub fn MoonBitPermissionAgent::negative_cases(
  self : Self,
  target_agent_id : String,
) -> Result[Array[String], String] {
  ignore(self)
  let observations = []
  let root = require_template_root()
  defer root.drop()
  try {
    let grant = inspect_grant(root)
    let narrow = @permissions.derive_from_wallet(
      inspect_bounds({ ..grant, recipient: "nobody@example.com/*/*/*/*", }),
    )
    try {
      narrow.install(self_holder(target_agent_id))
      observations.push("recipient-mismatch=accepted")
    } catch {
      error => observations.push("recipient-mismatch=\{error.kind()}")
    }
    observations.push("mismatched-card-moved=\{narrow.is_moved()}")
    let account = @permissions.derive_from_wallet(inspect_bounds(grant))
    try {
      account.install(Account({ high_bits: 0, low_bits: 0, }))
      observations.push("account-target=accepted")
    } catch {
      error => observations.push("account-target=\{error.kind()}")
    }
    try {
      @permissions.validate_grant({ ..grant, verb: "fly", })
      observations.push("invalid-verb=accepted")
    } catch {
      error => observations.push("invalid-verb=\{error.kind()}")
    }
    Ok(observations)
  } catch {
    error => Err(error.to_string())
  }
}

///|
/// Step 7: requests a card for every card verb with any owner, which the wallet
/// does not cover. Returns the error kind, or `derived`.
pub fn MoonBitPermissionAgent::derive_beyond_wallet(self : Self) -> String {
  ignore(self)
  let grant : @permissions.PatternGrant = {
    class: "card",
    owner: "*",
    recipient: "*",
    verb: "*",
    resource_id: "*",
  }
  try {
    @permissions.derive_from_wallet(inspect_bounds(grant)).drop()
    "derived"
  } catch {
    error => error.kind()
  }
}

///|
/// Reads the template root's identifier. With `drop_first` the card is moved by
/// `drop` before the read, which aborts the invocation; without it the read
/// succeeds and returns `root-present`.
pub fn MoonBitPermissionAgent::use_after_move(
  self : Self,
  drop_first : Bool,
) -> String {
  ignore(self)
  let root = require_template_root()
  if drop_first {
    root.drop()
  }
  ignore(root.id())
  root.drop()
  "root-present"
}

///|
fn is_oplog_grant(grant : @permissions.PatternGrant) -> Bool {
  grant.class == "oplog"
}

///|
/// The template root's oplog grant, as `inspect` returns it. Its owner is the
/// `?agent` slot.
fn template_oplog_grant(
  root : @permissions.PermissionCard,
) -> @permissions.PatternGrant raise @permissions.PermissionError {
  match root.inspect().lower_positive.iter().find_first(is_oplog_grant) {
    Some(grant) => grant
    None => abort("the template root does not grant oplog access")
  }
}

///|
/// A concrete grant to read the oplog of the agent with the owner path
/// `owner` (`account/application/environment/component/agent`). A requested
/// grant must be concrete: the host rejects a slot variable such as `?agent`.
fn oplog_read_grant(
  root : @permissions.PermissionCard,
  owner : String,
) -> @permissions.PatternGrant raise @permissions.PermissionError {
  { ..template_oplog_grant(root), owner, verb: "read", resource_id: "*", }
}

///|
/// Step 4: derives a scope card from the template root that grants card
/// inspection and reading this agent's oplog, and lends it to `scope_report`
/// on `target`: per call, as the client default, and not at all. Only the
/// scope card lets the target read this agent's oplog. `owner` is this
/// agent's owner path. Requesting the inspected `?agent` template grant
/// unchanged is rejected first. Returns that rejection, the three reports and
/// whether the caller kept the scope card.
pub async fn MoonBitPermissionAgent::scope_rpc(
  self : Self,
  target : String,
  owner : String,
) -> Result[Array[String], String] {
  ignore(self)
  let root = require_template_root()
  defer root.drop()
  let caller = @api.get_self_metadata().agent_id.agent_id
  let template_grant = try {
    let bounds = @permissions.Bounds::new(lower_positive=[
      template_oplog_grant(root),
    ])
    @permissions.derive_scope([root], bounds).drop()
    "accepted"
  } catch {
    error => error.kind()
  }
  let scope = try {
    let bounds = @permissions.Bounds::new(lower_positive=[
      inspect_grant(root),
      oplog_read_grant(root, owner),
    ])
    @permissions.derive_scope([root], bounds)
  } catch {
    error => return Err(error.to_string())
  }
  defer scope.drop()
  let client = MoonBitPermissionAgentClient::get(target) catch {
    error => return Err(repr(error))
  }
  defer client.drop()
  let per_call = client.scope_report(caller, scope_card=scope)
  client.set_default_scope_card(Some(scope))
  let by_default = client.scope_report(caller)
  client.set_default_scope_card(None)
  let without = client.scope_report(caller)
  Ok([
    "template-grant=\{template_grant}",
    "per-call=\{per_call}",
    "default=\{by_default}",
    "without=\{without}",
    "scope-kept=\{!scope.is_moved()}",
  ])
}

///|
/// Describes the scope card that the caller lent to this invocation as
/// `present/<parent is the root>/<lower-positive grant count>`, or `absent`,
/// followed by `/readable` when this agent can read the oplog of the agent
/// `viewed` of this component and `/denied` otherwise.
pub fn MoonBitPermissionAgent::scope_report(
  self : Self,
  viewed : String,
) -> String {
  let view = if self.can_read_oplog(viewed) { "readable" } else { "denied" }
  let cards = @permissions.wallet()
  defer @permissions.drop_all(cards)
  let root_ids = []
  for card in cards {
    let is_template = card.inspect().lower_positive
      .iter()
      .any(is_template_grant) catch {
        _ => false
      }
    if is_template {
      root_ids.push(card.id())
    }
  }
  for card in cards {
    let parents = card.parents()
    if parents.is_empty() {
      continue
    }
    let parent_is_root = parents
      .iter()
      .any(fn(parent) { root_ids.contains(parent) })
    let grants = card.inspect().lower_positive.length() catch { _ => -1 }
    return "present/\{parent_is_root}/\{grants}/\{view}"
  }
  "absent/\{view}"
}

///|
/// The number of cards in this agent's wallet.
pub fn MoonBitPermissionAgent::wallet_card_count(self : Self) -> UInt {
  ignore(self)
  let cards = @permissions.wallet()
  @permissions.drop_all(cards)
  cards.length().reinterpret_as_uint()
}

///|
/// Step 5: derives a card that grants card inspection and reading this agent's
/// oplog from the wallet, and installs it on `target_agent_id` in this
/// component. `owner` is this agent's owner path. Returns whether installing
/// moved the card.
pub fn MoonBitPermissionAgent::install_on(
  self : Self,
  target_agent_id : String,
  owner : String,
) -> Result[Bool, String] {
  ignore(self)
  let root = require_template_root()
  defer root.drop()
  try {
    let card = @permissions.derive_from_wallet(
      @permissions.Bounds::new(lower_positive=[
        inspect_grant(root),
        oplog_read_grant(root, owner),
      ]),
    )
    card.install(self_holder(target_agent_id))
    Ok(card.is_moved())
  } catch {
    error => Err(error.to_string())
  }
}

///|
/// Step 6: revokes the template root from the wallet and returns the number of
/// revoked cards.
pub fn MoonBitPermissionAgent::revoke_root(self : Self) -> Result[UInt, String] {
  ignore(self)
  let root = require_template_root()
  try {
    let count = root.revoke()
    guard root.is_moved() else { abort("revoke must move the card") }
    Ok(count)
  } catch {
    error => Err(error.to_string())
  }
}

///|
/// Step 6: revokes the card that `install_on` installed in this agent's
/// wallet. Returns the revoked-card count and whether revoking moved the card.
pub fn MoonBitPermissionAgent::revoke_installed_card(
  self : Self,
) -> Result[Array[String], String] {
  ignore(self)
  let cards = @permissions.wallet()
  defer @permissions.drop_all(cards)
  let installed = cards
    .iter()
    .find_first(fn(card) {
      !(card.inspect().lower_positive.iter().any(is_template_grant) catch {
        _ => true
      })
    })
  guard installed is Some(card) else { return Err("no installed card") }
  try {
    let count = card.revoke()
    Ok(["revoked=\{count}", "card-moved=\{card.is_moved()}"])
  } catch {
    error => Err(error.to_string())
  }
}

///|
/// Whether this agent can read the oplog of the agent `viewed` of this
/// component. The read starts past the end of the oplog, so it checks the
/// authority without projecting any entry.
pub fn MoonBitPermissionAgent::can_read_oplog(
  self : Self,
  viewed : String,
) -> Bool {
  ignore(self)
  let oplog = @oplogRaw.GetOplog::get_oplog(
    {
      component_id: @api.get_self_metadata().agent_id.component_id,
      agent_id: viewed,
    },
    1_000_000,
  )
  defer oplog.drop()
  oplog.get_next() is Ok(_)
}

///|
/// Derives an inspection card from the wallet's template root. The host denies
/// the derivation when the wallet lost its derive authority.
pub fn MoonBitPermissionAgent::derive_from_wallet_kind(self : Self) -> String {
  ignore(self)
  let grant : @permissions.PatternGrant = {
    class: "card",
    owner: "*",
    recipient: "*",
    verb: "inspect",
    resource_id: "*",
  }
  try {
    @permissions.derive_from_wallet(inspect_bounds(grant)).drop()
    "derived"
  } catch {
    error => error.kind()
  }
}
