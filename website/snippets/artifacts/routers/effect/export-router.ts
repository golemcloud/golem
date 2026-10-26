// asked to export results as CSV, the agent replies with a link:
// the router sets the status, content type and download headers:
export const Exports = HttpRouter.define("ExportsRouter", {
  mount: Http.mount("/exports"),
})
Exports.implement(Effect.succeed(Effect.gen(function* () {
  const request = yield* HttpRouter.request
  const path = /^\/exports\/([\w-]+)\/([\w-]{36})\.csv$/
  const m = request.path.match(path) //@
  if (request.method !== "GET" || !m) { //@
    return HttpServerResponse.empty({ status: 404 }) //@
  }
  // that user's agent builds the CSV now and streams it:
  const analyst = yield* Analyst.client.get({ chat: m[1] }) //@
  const csv = yield* analyst.exportCsv({ id: m[2] })
  return HttpRouter.withRawHeaders(
    HttpServerResponse.stream(csv, { status: 200 }), [ //@
      header("content-type", "text/csv"), //@
      // the browser saves it as a download:
      header("content-disposition", //@
        'attachment; filename="results.csv"'), //@
    ])
})))
