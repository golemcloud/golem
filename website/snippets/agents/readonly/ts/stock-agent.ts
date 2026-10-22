export const Stock = defineAgent({
  name: 'StockAgent',
  id: { sku: z.string() },
  methods: {
    restock: method({ input: { n: z.int() }, returns: z.void() }),
    level: method({
      input: {}, returns: z.int(),
      // repeat calls come from a cache, without loading the agent:
      // the cache clears when a restock succeeds:
      // outgoing HTTP, agent calls and key-value writes fail:
      readOnly: true, //@
    }),
  },
}).implement({
  init: () => ({ units: 0 }),
  methods: {
    restock({ n }) { this.units += n },
    level() { return this.units },
  },
})
