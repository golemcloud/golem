HttpRouter.define("ExportsRouter", {
  mount: Http.mount("/exports"),
}).implement(Effect.succeed(Effect.gen(function* () {
  const request = yield* HttpRouter.request
  const id = request.path.match(/^\/exports\/([\w-]+)\.csv$/)?.[1]
  if (request.method !== "GET" || !id) {
    return HttpServerResponse.empty({ status: 404 })
  }
  // stream the CSV from the agent that builds this export:
  const exp = yield* Export.client.get({ id }) //@
  const rows = yield* exp.csv({}) //@
  return HttpServerResponse.stream(rows, {
    status: 200,
    contentType: "text/csv", //@
    headers: {
      // browsers save it as a file instead of opening it:
      "content-disposition": //@
        `attachment; filename="${id}.csv"`, //@
    },
  })
})))
