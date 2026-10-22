// define a grant for connections to the orders API only:
var ordersAPI = golem.PatternGrant{
    Class: "network", Owner: "", Recipient: "*",
    Verb: "connect", ResourceID: "orders.example.com:443", //@
}

func ShareOrders(target golem.AgentID) golem.PersistentCard {
    bounds := golem.Bounds{
        LowerPositive: []golem.PatternGrant{ordersAPI}}
    parent := golem.MustDeriveFromWallet(
        bounds, golem.None[golem.Lifespan]())
    // install a one-hour child; keep its parent for revocation:
    child := parent.MustDerive(
        bounds, golem.Some(golem.LifespanOf(time.Hour))) //@
    golem.Must0(child.Install(golem.AgentHolder(target))) //@
    return parent
}

func RevokeOrders(parent golem.PersistentCard) {
    // revoking it revokes the child and every card derived from it:
    golem.Must(parent.Revoke()) //@
}
