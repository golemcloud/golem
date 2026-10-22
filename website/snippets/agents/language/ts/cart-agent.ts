const Item = z.object({ sku: z.string(), quantity: z.int() })
type Item = z.infer<typeof Item>

// the first call creates one durable CartAgent per user:
export const Cart = defineAgent({
  name: 'CartAgent',
  id: { user: z.string() },
  methods: {
    add: method({ input: { item: Item }, returns: z.int() }), //@
    checkout: method({ input: {}, returns: z.string() }),
  },
}).implement({
  init: ({ id }) => ({ user: id.user, items: [] as Item[] }),
  methods: {
    add({ item }) {
      this.items.push(item) //@
      return this.items.length
    },
    async checkout() {
      const order = await placeOrder(this.user, this.items)
      this.items = []
      return order.id
    },
  },
})
