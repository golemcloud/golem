const bytes = (text: string) => new TextEncoder().encode(text)

export const Exports = defineHttpRouter('ExportsRouter')
  .mount('/exports')
  .implementRaw(async (request) => {
    const id = request.path.match(/^\/exports\/([\w-]+)\.csv$/)?.[1]
    if (request.method !== 'GET' || !id) {
      const body = AgentStream.from<Uint8Array>([])
      return { status: 404, headers: [], body }
    }
    // stream the CSV from the agent that builds this export:
    const rows = await Export.client.get({ id }).csv() //@
    return {
      status: 200,
      headers: [
        { name: 'content-type', value: bytes('text/csv') }, //@
        // browsers save it as a file instead of opening it:
        { name: 'content-disposition', value: bytes( //@
          `attachment; filename="${id}.csv"`) }, //@
      ],
      body: rows,
    }
  })
