// this router serves exports from durable agents:
HttpRouter.define("ExportsRouter", {
  mount: Http.mount("/exports"),
}).implement(Effect.succeed(Effect.gen(function* () {
  const request = yield* HttpRouter.request
  const match = request.path.match(/^\/exports\/([\w-]+)\.csv$/) //@
  if (request.method !== "GET" || !match) { //@
    return HttpServerResponse.empty({ status: 404 }) //@
  }
  // the durable agent behind each export streams its CSV:
  const exp = yield* Export.client.get({ id: match[1] }) //@
  const rows = yield* exp.csv({}) //@
  return HttpServerResponse.stream(rows, { //@
    contentType: "text/csv",
    headers: { "content-disposition": "attachment" }, //@
  })
})))
