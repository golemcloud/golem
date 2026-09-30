# Durable Streams walkthrough (MoonBit)

Set `DURABLE_STREAM_TOKEN` before deployment. `Secret::with_handle` borrows the opaque host capability only while each Durable Streams resource is constructed; it never reveals or logs plaintext.

```sh
export DURABLE_STREAM_TOKEN=... # do not commit this value
golem deploy -L -Y
```

## HTTP stream, result, and fork

The `durable_echo` endpoint exposes its `input` and `$result` slots as Durable Streams. Replace the origin if your local HTTP API uses another port.

```sh
ORIGIN=http://app-name.localhost:9006
SESSION=$(uuidgen)
BASE="$ORIGIN/durable-stream-agents/demo/echo/invocations/$SESSION"
INPUT="$BASE/streams/input"
RESULT="$BASE/streams/\$result"

curl -i -X PUT "$INPUT"
curl -i -X POST -H 'content-type: application/json' --data '"shared"' "$INPUT"

FORK_INPUT="${INPUT/\/invocations\//\/forks\/demo-fork\/invocations\/}"
curl -i -X PUT -H "stream-forked-from: ${INPUT#"$ORIGIN"}" "$FORK_INPUT"
curl -i -X POST -H 'content-type: application/json' -H 'stream-closed: true' --data '"source-only"' "$INPUT"
curl -i -X POST -H 'content-type: application/json' -H 'stream-closed: true' --data '"fork-only"' "$FORK_INPUT"
curl -sS "$INPUT?offset=-1&live=false"
curl -sS "$FORK_INPUT?offset=-1&live=false"
curl -sS "$RESULT?offset=-1&live=false" # ["echo:shared","echo:source-only"]
curl -sS "${RESULT/\/invocations\//\/forks\/demo-fork\/invocations\/}?offset=-1&live=false"
```

## External Durable Stream

In another terminal, start the external reference server pinned by this example:

```sh
mkdir -p /tmp/durable-stream-reference && cd /tmp/durable-stream-reference
npm install --no-save --save-exact @durable-streams/server@0.3.9
node --input-type=module <<'EOF'
import { DurableStreamTestServer } from "@durable-streams/server"
const server = new DurableStreamTestServer({ host: "127.0.0.1", port: 7878 })
console.log(await server.start())
await new Promise(() => {})
EOF
```

Then create a JSON stream and exercise idempotent append, close, and read. The second append reuses the immutable producer sequence-zero tuple, so it is acknowledged without adding data:

```sh
STREAM_URL=http://127.0.0.1:7878/example
curl -i -X PUT -H 'content-type: application/json' "$STREAM_URL"
golem agent invoke 'StreamingAgent("external")' append_external "\"$STREAM_URL\"" '"stable-producer"' '["once"]' false --no-stream
golem agent invoke 'StreamingAgent("external")' append_external "\"$STREAM_URL\"" '"stable-producer"' '["once"]' false --no-stream # None: duplicate
golem agent invoke 'StreamingAgent("external")' append_external "\"$STREAM_URL\"" '"closer"' '["tail"]' true --no-stream
golem agent invoke 'StreamingAgent("external")' read_external "\"$STREAM_URL\"" --no-stream # ["once","tail"]
```

See `streaming_agent.mbt` for the complete SDK calls.
