export const Reports = defineAgent({
  name: 'ReportAgent',
  id: { team: z.string() },
  http: http.mount('/reports/{team}', {
    // Golem serves these live from this agent's own file system:
    exposeFiles: [
      { route: '/', path: '/site/index.html' }, //@
      { route: '/*', path: '/site/$1' }, //@
    ],
  }),
  methods: {
    build: method({
      input: { month: z.string() }, returns: z.void(),
      http: http.post('/build/{month}'), //@
    }),
  },
}).implement({
  init: ({ id }) => {
    // the agent writes a front-end for its own team:
    fs.mkdirSync('/site', { recursive: true })
    fs.writeFileSync('/site/index.html', frontEnd(id.team)) //@
    return {}
  },
  methods: {
    async build({ month }) {
      // it is served at /report.html once this call returns:
      fs.writeFileSync('/site/report.html', await render(month))
    },
  },
})
