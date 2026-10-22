const Item = Schema.Struct({
  sku: Schema.String, quantity: Schema.Int,
})
type Item = typeof Item.Type

// the first call creates one durable CartAgent per user:
export const Cart = defineAgent({
  name: "CartAgent",
  id: { user: Schema.String },
  methods: {
    add: method({ input: { item: Item }, success: Schema.Int }), //@
    checkout: method({ input: {}, success: Schema.String }),
  },
}).implement({
  init: ({ user }) => Effect.succeed({ user, items: [] as Item[] }),
  methods: (cart) => ({
    add: ({ item }) => Effect.sync(() => {
      cart.items.push(item) //@
      return cart.items.length
    }),
    checkout: () => Effect.gen(function* () {
      const order = yield* placeOrder(cart.user, cart.items)
      cart.items = []
      return order.id
    }),
  }),
})
