// this router serves exports from durable agents:
export const Exports = defineHttpRouter('ExportsRouter')
  .mount('/exports')
  .implementRaw(async (request) => {
    const match = request.path.match(/^\/exports\/([\w-]+)\.csv$/) //@
    if (request.method !== 'GET' || !match) { //@
      return { status: 404, headers: [], body: empty() } //@
    }
    // the durable agent behind each export streams its CSV:
    const rows = await Export.client.get({ id: match[1] }).csv() //@
    return {
      status: 200,
      headers: [
        header('content-type', 'text/csv'),
        header('content-disposition', 'attachment'), //@
      ],
      body: rows, //@
    }
  })
