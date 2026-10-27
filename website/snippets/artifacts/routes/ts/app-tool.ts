// AppAgent's model calls this tool to save an app it just wrote:
export const present = toolDefinition('present', {
  requiresFilesystem: true,
}).body((b) => b.positional('name', Name)
  .positional('html', z.string()).returns(z.string()))
present.implement({
  present: async ({ name, html }) => {
    // the app is saved in the calling agent's own files:
    fs.mkdirSync('/out', { recursive: true })
    fs.writeFileSync(`/out/${name}.html`, html) //@
    return ok(`${name}.html`)
  },
})
export const Apps = defineAgent({
  name: 'AppAgent',
  id: { chat: z.string() },
  http: http.mount('/apps/{chat}', {
    exposeFiles: [{ route: '/*', path: '/out/$1' }],
  }),
  methods: {
    sales: method({
      input: { region: z.string() }, returns: z.array(Row),
      // the app's buttons fetch('sales?region=...') from here:
      http: http.get('/sales?region={region}'), //@
    }),
  },
}).implement({
  init: () => ({}),
  // each click runs this as durable code in the agent's own VM:
  methods: { sales: ({ region }) => querySales(region) }, //@
})
