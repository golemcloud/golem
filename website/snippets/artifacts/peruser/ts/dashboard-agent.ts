export const Dashboards = defineAgent({
  name: 'DashboardAgent',
  id: { user: z.string() },
  // the path names the agent: /dashboards/ana is Ana's own agent:
  http: http.mount('/dashboards/{user}', { //@
    auth: true,
    exposeFiles: [{ route: '/*', path: '/out/$1' }],
  }),
  methods: {
    ask: method({ input: { request: z.string() },
      returns: z.string(), http: http.post('/ask') }),
  },
}).implement({
  // the first request to /dashboards/ana creates her agent:
  init: ({ id }) => ({ user: id.user }), //@
  methods: {
    async ask({ request }) {
      const name = crypto.randomUUID()
      fs.mkdirSync('/out', { recursive: true })
      // the model builds this dashboard for her one request:
      fs.writeFileSync(`/out/${name}.html`,
        await dashboardFor(this.user, request))
      // what it made for her is served under her path:
      return `/dashboards/${this.user}/${name}.html` //@
    },
  },
})
