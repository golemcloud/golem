export const Stock = defineAgent({
  name: "StockAgent",
  id: { sku: Schema.String },
  methods: {
    restock: method({
      input: { n: Schema.Int }, success: Schema.Void,
    }),
    level: method({
      input: {}, success: Schema.Int,
      // repeat calls come from a cache, without loading the agent:
      // the cache clears when a restock succeeds:
      // outgoing HTTP, agent calls and key-value writes fail:
      readOnly: true, //@
    }),
  },
}).implement({
  init: () => Ref.make(0),
  methods: (units) => ({
    restock: ({ n }) => Ref.update(units, (u) => u + n),
    level: () => Ref.get(units),
  }),
})
