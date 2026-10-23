// asked to export results as CSV, the agent replies with a link:
// the link is /exports/<chat>/<random id>.csv, for that user only:
export const Exports = defineHttpRouter('ExportsRouter')
  .mount('/exports')
  .implementRaw(async (request) => {
    const path = /^\/exports\/([\w-]+)\/([\w-]{36})\.csv$/
    const m = request.path.match(path) //@
    if (request.method !== 'GET' || !m) { //@
      return { status: 404, headers: [], body: empty() } //@
    }
    // that user's agent builds the CSV now and streams it:
    const csv = await Analyst.client.get({ chat: m[1] }) //@
      .exportCsv({ id: m[2] })
    return {
      status: 200,
      headers: [
        header('content-type', 'text/csv'), //@
        // the browser saves it as a download:
        header('content-disposition', //@
          'attachment; filename="results.csv"'), //@
      ],
      body: csv, //@
    }
  })
