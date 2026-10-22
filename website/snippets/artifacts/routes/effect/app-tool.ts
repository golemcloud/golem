// AppAgent's model calls this tool to save an app it just wrote:
export const present = Tool.toolDefinition("present", {
  requiresFilesystem: true,
}).body((b) => b.positional("name", Name)
  .positional("html", Schema.String).returns(Schema.String))
present.implement({
  present: ({ name, html }) => Effect.sync(() => {
    // the app is saved in the calling agent's own files:
    fs.mkdirSync("/out", { recursive: true })
    fs.writeFileSync(`/out/${name}.html`, html) //@
    return Tool.ok(`${name}.html`)
  }),
})

export const Apps = defineAgent({
  name: "AppAgent",
  id: { chat: Schema.String },
  http: Http.mount("/apps/{chat}", {
    exposeFiles: [{ route: "/*", path: "/out/$1" }],
  }),
  methods: {
    sales: method({
      input: { region: Schema.String }, success: Schema.Array(Row),
      // the app's buttons fetch('sales?region=...') from here:
      http: [Http.get("/sales?region={region}")], //@
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({ sales: ({ region }) => querySales(region) }),
})
