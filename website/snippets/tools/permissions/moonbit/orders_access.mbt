// define a grant for connections to the orders API only:
let orders_api : @permissions.PatternGrant = {
  class: "network", owner: "", recipient: "*",
  verb: "connect", resource_id: "orders.example.com:443", //@
}

pub fn share_orders(
  target : @types.AgentId,
) -> @permissions.PersistentCard raise {
  let bounds = @permissions.Bounds::new(lower_positive=[orders_api])
  let parent = @permissions.derive_from_wallet(bounds)
  // install a one-hour child; keep its parent for revocation:
  let child = parent.derive_child(
    bounds, lifespan=@permissions.Lifespan::iso8601("PT1H"), //@
  )
  child.install(Agent(target)) //@
  parent
}

pub fn revoke_orders(
  parent : @permissions.PersistentCard,
) -> Unit raise {
  // revoking it revokes the child and every card derived from it:
  ignore(parent.revoke()) //@
}
