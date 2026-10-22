type Item struct {
    SKU      string
    Quantity int32
}
type CartID struct{ User string }
type AddIn struct{ Item Item }

// the first call creates one durable CartAgent per user:
var Cart = golem.DefineAgent[CartID](golem.Spec{Name: "CartAgent"})
var (
    Add      = Cart.Method[AddIn, int32]("add") //@
    Checkout = Cart.Method[golem.Unit, string]("checkout")
)

type cart struct {
    user  string
    items []Item
}

var impl = Cart.Implement(func(id CartID) *cart {
    return &cart{user: id.User}
})
var _ = impl.Handle(Add, golem.Adapt(func(c *cart, in AddIn) int32 {
    c.items = append(c.items, in.Item) //@
    return int32(len(c.items))
}))
var _ = impl.Handle(Checkout, golem.Adapt0(func(c *cart) string {
    order := placeOrder(c.user, c.items)
    c.items = nil
    return order.ID
}))
