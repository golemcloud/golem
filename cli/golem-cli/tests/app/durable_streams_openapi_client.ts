import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import createClient from "openapi-fetch";
import type { paths } from "./schema.js";

const client = createClient<paths>({ baseUrl: process.argv[2] });
const identity = () => ({ id: randomUUID(), session: randomUUID() });

// Body-bound creation, typed JSON append, atomic closure, and cursor-based reads.
const path = identity();
const created = await client.PUT(
  "/durable-stream-agents/{id}/prefixed/invocations/{session}",
  {
    params: { path },
    body: { prefix: "prefix:" },
  },
);
assert.equal(created.response.status, 201);
const appended = await client.POST(
  "/durable-stream-agents/{id}/prefixed/invocations/{session}/streams/input",
  {
    params: {
      path,
      header: {
        "Stream-Closed": "true",
        "Producer-Id": "generated",
        "Producer-Epoch": 0,
        "Producer-Seq": 0,
      },
    },
    body: ["alpha", "second"],
  },
);
assert.equal(appended.response.status, 200);
assert.equal(appended.response.headers.get("producer-seq"), "0");
assert.equal(appended.response.headers.get("stream-closed"), "true");
let offset = "-1";
let closed = false;
const values: unknown[] = [];
for (let attempt = 0; attempt < 20; attempt++) {
  const read = await client.GET(
    "/durable-stream-agents/{id}/prefixed/invocations/{session}/streams/%24result",
    {
      params: { path, query: { offset, live: "long-poll" } },
    },
  );
  assert.ok(
    [200, 204].includes(read.response.status),
    `read status ${read.response.status}`,
  );
  if (read.response.status === 200) {
    assert.ok(Array.isArray(read.data));
    values.push(...read.data);
  }
  offset = read.response.headers.get("stream-next-offset")!;
  assert.match(offset, /^[0-9a-f]{48}$/);
  closed = read.response.headers.get("stream-closed") === "true";
  if (closed) break;
}
assert.ok(closed, "JSON stream never reached EOF");
assert.deepEqual(values, ["prefix:alpha", "prefix:second"]);
const eof = await client.GET(
  "/durable-stream-agents/{id}/prefixed/invocations/{session}/streams/%24result",
  {
    params: { path, query: { offset, live: "long-poll" } },
  },
);
assert.equal(eof.response.status, 204);
assert.equal(eof.response.headers.get("stream-closed"), "true");

// Record elements must retain their field types in generated definitions.
type RecordAppend = NonNullable<
  paths["/durable-stream-agents/{id}/echo-records/invocations/{session}/streams/input"]["post"]["requestBody"]
>["content"]["application/json"];
// @ts-expect-error The generated record schema must reject a string-valued number.
const rejectedRecord: RecordAppend = { name: "bad", number: "not a number" };
void rejectedRecord;
const records = identity();
assert.equal(
  (
    await client.PUT(
      "/durable-stream-agents/{id}/echo-records/invocations/{session}",
      {
        params: { path: records },
      },
    )
  ).response.status,
  201,
);
assert.equal(
  (
    await client.POST(
      "/durable-stream-agents/{id}/echo-records/invocations/{session}/streams/input",
      {
        params: { path: records, header: { "Stream-Closed": "true" } },
        body: [
          { name: "first", number: 7 },
          { name: "second", number: 19 },
        ],
      },
    )
  ).response.status,
  204,
);
const recordValues: unknown[] = [];
offset = "-1";
closed = false;
for (let attempt = 0; attempt < 20; attempt++) {
  const read = await client.GET(
    "/durable-stream-agents/{id}/echo-records/invocations/{session}/streams/%24result",
    {
      params: { path: records, query: { offset, live: "long-poll" } },
    },
  );
  assert.ok([200, 204].includes(read.response.status));
  if (read.response.status === 200) {
    assert.ok(Array.isArray(read.data));
    recordValues.push(...read.data);
  }
  offset = read.response.headers.get("stream-next-offset")!;
  closed = read.response.headers.get("stream-closed") === "true";
  if (closed) break;
}
assert.ok(closed, "Record stream never reached EOF");
assert.deepEqual(recordValues, [
  { name: "first", number: 7 },
  { name: "second", number: 19 },
]);

// Binary reads use the generated operation with its supported parse mode.
const binary = identity();
assert.equal(
  (
    await client.PUT(
      "/durable-stream-agents/{id}/bytes/{count}/invocations/{session}",
      {
        params: { path: { ...binary, count: 5 }, query: { delay_ms: 0 } },
      },
    )
  ).response.status,
  201,
);
const bytes: number[] = [];
offset = "-1";
closed = false;
for (let attempt = 0; attempt < 20; attempt++) {
  const read = await client.GET(
    "/durable-stream-agents/{id}/bytes/{count}/invocations/{session}/streams/%24result",
    {
      params: {
        path: { ...binary, count: 5 },
        query: { offset, live: "long-poll" },
      },
      parseAs: "arrayBuffer",
    },
  );
  assert.ok([200, 204].includes(read.response.status));
  if (read.response.status === 200) bytes.push(...new Uint8Array(read.data!));
  offset = read.response.headers.get("stream-next-offset")!;
  closed = read.response.headers.get("stream-closed") === "true";
  if (closed) break;
}
assert.ok(closed, "Byte stream never reached EOF");
assert.deepEqual(bytes, [0, 1, 2, 3, 4]);

// Server-assigned session IDs are discovered through the documented Location.
const assigned = await client.PUT("/durable-stream-agents/{id}/json/{count}", {
  params: { path: { id: randomUUID(), count: 2 }, query: { delay_ms: 0 } },
});
assert.equal(assigned.response.status, 201);
assert.match(
  assigned.response.headers.get("location")!,
  /\/invocations\/[A-Za-z0-9._-]+$/,
);
console.log("generated-durable-stream-client-ok");
