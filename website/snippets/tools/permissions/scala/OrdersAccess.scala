// define a grant for connections to the orders API only:
val ordersApi = PermissionsApi.PatternGrant(
  className = "network", owner = "", recipient = "*",
  verb = "connect", resourceId = "orders.example.com:443", //@
)

def shareOrders(target: AgentId): PermissionsApi.PersistentCard =
  val bounds =
    PermissionsApi.Bounds(lowerPositive = List(ordersApi))
  val parent = PermissionsApi.deriveFromWallet(bounds).orThrow
  // install a one-hour child; keep its parent for revocation:
  val child = parent.derive(
    bounds, Some(PermissionsApi.Lifespan.iso8601("PT1H")), //@
  ).orThrow
  child.install(PermissionsApi.Holder.Agent(target)).orThrow //@
  parent

def revokeOrders(parent: PermissionsApi.PersistentCard): Unit =
  // revoking it revokes the child and every card derived from it:
  parent.revoke().orThrow //@
