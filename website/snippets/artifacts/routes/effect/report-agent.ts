export const Reports = defineAgent({
  name: "ReportAgent",
  id: { team: Schema.String },
  http: Http.mount("/reports/{team}", {
    // Golem serves these live from this agent's own file system:
    exposeFiles: [
      { route: "/", path: "/site/index.html" }, //@
      { route: "/*", path: "/site/$1" }, //@
    ],
  }),
  methods: {
    build: method({
      input: { month: Schema.String }, success: Schema.Void,
      http: [Http.post("/build/{month}")], //@
    }),
  },
}).implement({
  init: ({ team }) => Effect.sync(() => {
    // the agent writes a front-end for its own team:
    fs.mkdirSync("/site", { recursive: true })
    fs.writeFileSync("/site/index.html", frontEnd(team)) //@
  }),
  methods: () => ({
    build: ({ month }) => Effect.gen(function* () {
      // it is served at /report.html once this call returns:
      fs.writeFileSync("/site/report.html", yield* render(month))
    }),
  }),
})
