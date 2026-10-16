import { writeFile } from "node:fs/promises"
import { createServer, request as forward } from "node:http"
import { DurableStreamTestServer } from "@durable-streams/server"

const server = new DurableStreamTestServer({
  host: "127.0.0.1",
  port: 0,
  longPollTimeout: 250,
})
const upstream = await server.start()
const requests = []
const expectedToken = process.env.GOLEM_DS_TEST_TOKEN

// Record transport selection without interpreting or changing upstream stream data.
const proxy = createServer((req, res) => {
  const url = new URL(req.url, upstream)
  if (url.pathname === "/__golem_test/requests") {
    res.writeHead(200, { "content-type": "application/json" })
    res.end(JSON.stringify(requests))
    return
  }
  const authenticated = req.headers.authorization === `Bearer ${expectedToken}`
  const record = {
    method: req.method,
    path: url.pathname,
    live: url.searchParams.get("live"),
    producerId: req.headers["producer-id"],
    producerEpoch: req.headers["producer-epoch"],
    producerSeq: req.headers["producer-seq"],
    closed: req.headers["stream-closed"],
    authenticated,
  }
  if (url.pathname === "/template" && !authenticated) {
    record.status = 401
    requests.push(record)
    res.writeHead(401)
    res.end()
    return
  }
  const outgoing = forward(url, { method: req.method, headers: req.headers }, (incoming) => {
    record.status = incoming.statusCode
    res.writeHead(incoming.statusCode, incoming.headers)
    incoming.pipe(res)
  })
  outgoing.once("finish", () => requests.push(record))
  outgoing.once("error", () => {
    if (!res.headersSent) res.writeHead(502)
    res.end()
  })
  res.once("close", () => outgoing.destroy())
  req.pipe(outgoing)
})
await new Promise((resolve) => proxy.listen(0, "127.0.0.1", resolve))
await writeFile(process.argv[2], JSON.stringify({ url: `http://127.0.0.1:${proxy.address().port}` }))

let stopping = false
async function stop() {
  if (stopping) return
  stopping = true
  await server.stop()
  proxy.closeAllConnections()
  await new Promise((resolve) => proxy.close(resolve))
  process.exit(0)
}

process.once("SIGTERM", stop)
process.once("SIGINT", stop)
process.stdin.once("end", stop)
process.stdin.resume()
