// define a grant for connections to the orders API only:
const ordersApi: Permissions.PatternGrant = {
  class: "network", owner: "", recipient: "*",
  verb: "connect", resourceId: "orders.example.com:443", //@
}

export const shareOrders = Effect.fn(function* (target: AgentId) {
  const bounds = { lowerPositive: [ordersApi] }
  const parent = yield* Permissions.deriveFromWallet(bounds)
  // install a one-hour child; keep its parent for revocation:
  const child = yield* Permissions.derive(
    parent, bounds, Permissions.Lifespan.fromIso8601("PT1H"), //@
  )
  yield* Permissions.install( //@
    child, Permissions.Holder.agent(target))
  return parent
})

export const revokeOrders = (parent: Card) =>
  // revoking it revokes the child and every card derived from it:
  Permissions.revoke(parent) //@
