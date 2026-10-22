// define a grant for connections to the orders API only:
const ordersApi: permissions.PatternGrant = {
  class: 'network', owner: '', recipient: '*',
  verb: 'connect', resourceId: 'orders.example.com:443', //@
}

export function shareOrders(target: AgentId) {
  const bounds = { lowerPositive: [ordersApi] }
  const parent = permissions.deriveFromWallet(bounds).unwrap()
  // install a one-hour child; keep its parent for revocation:
  const child = parent.derive(
    bounds, permissions.Lifespan.iso8601('PT1H'), //@
  ).unwrap()
  child.install(permissions.Holder.agent(target)).unwrap() //@
  return parent
}

export function revokeOrders(parent: Card) {
  // revoking it revokes the child and every card derived from it:
  parent.revoke().unwrap() //@
}
