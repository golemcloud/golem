export const Site = defineAgent({
  name: 'SiteAgent',
  id: { project: z.string() },
  http: http.mount('/sites/{project}', {
    exposeFiles: [{ route: '/', path: '/site/index.html' }], //@
  }),
  methods: {
    generate: method({
      input: { brief: z.string() }, returns: s.stream(z.string()), //@
      http: http.post('/generate'),
    }),
  },
}).implement({
  init: () => ({}),
  methods: {
    generate({ brief }) {
      return AgentStream.from((async function* () {
        let page = ''
        for await (const part of streamPage(brief)) {
          page += part
          yield part
        }
        fs.mkdirSync('/site', { recursive: true })
        // it's served at /sites/{project} once this call completes:
        fs.writeFileSync('/site/index.html', page) //@
      })())
    },
  },
})
