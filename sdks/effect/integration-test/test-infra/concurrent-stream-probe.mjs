import assert from "node:assert/strict"
import { randomUUID } from "node:crypto"
import { setTimeout as sleep } from "node:timers/promises"
import { WebSocketServer } from "ws"

// Run from sdks/effect/integration-test after building the SDK and installing this app's deps.
// Start a disposable server with router port 9882, custom request port 9008, and MCP port 9009:
//   golem -X server run --data-dir golem-temp/concurrent-stream-server \
//     --router-port 9882 --custom-request-port 9008 --mcp-port 9009
//   golem -L -Y -A concurrent-stream.golem.yaml build
//   golem -L -Y -A concurrent-stream.golem.yaml deploy
// Warm the component before applying the short stall timeout (cold compilation can take longer):
//   PROBE_TIMEOUT_MS=60000 node test-infra/concurrent-stream-probe.mjs 0 1
//   node test-infra/concurrent-stream-probe.mjs 2 10
// Readers 0 is a direct-echo control; 1 or 2 adds pending WebSocket readers.
// Failure exits nonzero and prints the fresh agent ID for `golem agent oplog` / `agent get`.
// The CLI integration suite runs this same probe with an isolated server via PROBE_BASE_URL.
const readers = Number(process.argv[2] ?? 2)
const runs = Number(process.argv[3] ?? 10)
const appendDelay = Number(process.env.PROBE_APPEND_DELAY_MS ?? 0)
const wakeDelay = Number(process.env.PROBE_WAKE_AFTER_MS ?? 0)
const timeout = Number(process.env.PROBE_TIMEOUT_MS ?? 10000)
const baseUrl = process.env.PROBE_BASE_URL ?? "http://localhost:9008"
assert([0, 1, 2].includes(readers))
assert(Number.isSafeInteger(runs) && runs > 0)
const peer = new WebSocketServer({ host: "127.0.0.1", port: 0 })
await new Promise((resolve, reject) => {
  peer.once("listening", resolve)
  peer.once("error", reject)
})
const peerPort = peer.address().port
let connections = 0
peer.on("connection", (socket, request) => {
  connections++
  console.log(JSON.stringify({ event: "connected", path: request.url }))
  if (request.url === "/first") socket.send("initial frame")
  if (wakeDelay > 0) {
    const timer = setTimeout(() => {
      console.log(JSON.stringify({ event: "wake", path: request.url }))
      socket.send("wake frame")
    }, wakeDelay)
    socket.once("close", () => clearTimeout(timer))
  }
  socket.on("message", (data, binary) => {
    console.log(JSON.stringify({ event: "received", binary, data: data.toString() }))
  })
})

async function request(url, init = {}) {
  const response = await fetch(url, { ...init, signal: AbortSignal.timeout(timeout) })
  const body = await response.text()
  assert(response.ok, `${response.status} ${url}: ${body}`)
  return { response, body }
}

try {
  for (let run = 1; run <= runs; run++) {
    const name = randomUUID()
    const agent = `ConcurrentStreamProbe("${name}",${readers},${peerPort})`
    const base = `${baseUrl}/stream-probe/${name}/${readers}/${peerPort}/echo/invocations/probe`
    console.log(JSON.stringify({ event: "start", run, readers, agent }))
    await request(`${base}/streams/input`, { method: "PUT" })
    if (appendDelay > 0) await sleep(appendDelay)
    await request(`${base}/streams/input`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "producer-id": "probe",
        "producer-epoch": "0",
        "producer-seq": "0",
        "stream-closed": "true",
      },
      body: JSON.stringify(["hello"]),
    })
    const deadline = Date.now() + timeout
    let echoed = false
    while (Date.now() < deadline) {
      const { response, body } = await request(`${base}/streams/$result?offset=-1`)
      const values = JSON.parse(body)
      if (values.length > 0) {
        assert.deepEqual(values, ["hello"])
        console.log(
          JSON.stringify({
            event: "echo",
            run,
            agent,
            closed: response.headers.get("stream-closed"),
          }),
        )
        echoed = true
        break
      }
      await sleep(25)
    }
    assert(echoed, `No echo within ${timeout} ms: ${agent}`)
    assert.equal(connections, readers * run, "Expected WebSocket readers were not connected")
  }
} catch (error) {
  console.error(error)
  await sleep(Number(process.env.PROBE_HOLD_MS ?? 0))
  process.exitCode = 1
} finally {
  for (const socket of peer.clients) socket.terminate()
  await new Promise((resolve) => peer.close(resolve))
}
