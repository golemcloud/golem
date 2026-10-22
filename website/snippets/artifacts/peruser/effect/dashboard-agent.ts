export const Dashboards = defineAgent({
  name: "DashboardAgent",
  id: { user: Schema.String },
  // the path names the agent: /dashboards/ana is Ana's own agent:
  http: Http.mount("/dashboards/{user}", { //@
    exposeFiles: [{ route: "/*", path: "/out/$1" }],
  }),
  methods: {
    ask: method({ input: { request: Schema.String },
      success: Schema.String, http: [Http.post("/ask")] }),
  },
}).implement({
  // the first request to /dashboards/ana creates her agent:
  init: ({ user }) => Effect.succeed({ user, made: 0 }), //@
  methods: (self) => ({
    ask: ({ request }) => Effect.gen(function* () {
      const n = ++self.made
      fs.mkdirSync("/out", { recursive: true })
      // the model builds this dashboard for her one request:
      fs.writeFileSync(`/out/${n}.html`,
        yield* dashboardFor(self.user, request))
      // what it made for her is served under her path:
      return `/dashboards/${self.user}/${n}.html` //@
    }),
  }),
})
