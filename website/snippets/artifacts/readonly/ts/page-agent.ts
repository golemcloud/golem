export const Page = defineAgent({
  name: 'PageAgent',
  id: { page: z.string() },
  http: http.mount('/pages/{page}'),
  methods: {
    publish: method({
      input: { html: z.string() }, returns: z.void(),
      http: http.put('/'),
    }),
    read: method({
      input: {}, returns: z.string(),
      // outgoing HTTP, RPC, key-value and blob writes trap:
      // publish clears the cache when it succeeds:
      // cache hits do not wake the agent:
      readOnly: true, //@
      http: http.get('/'),
    }),
  },
}).implement({
  init: () => ({ html: '' }),
  methods: {
    publish({ html }) { this.html = html },
    read() { return this.html },
  },
})
