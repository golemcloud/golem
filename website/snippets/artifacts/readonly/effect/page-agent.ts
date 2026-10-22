export const Page = defineAgent({
  name: "PageAgent",
  id: { page: Schema.String },
  http: Http.mount("/pages/{page}"),
  methods: {
    publish: method({
      input: { html: Schema.String }, success: Schema.Void,
      http: [Http.put("/")],
    }),
    read: method({
      input: {}, success: Schema.String,
      // outgoing HTTP, RPC, key-value and blob writes trap:
      // publish clears the cache when it succeeds:
      // cache hits do not wake the agent:
      readOnly: true, //@
      http: [Http.get("/")],
    }),
  },
}).implement({
  init: () => Ref.make(""),
  methods: (page) => ({
    publish: ({ html }) => Ref.set(page, html),
    read: () => Ref.get(page),
  }),
})
