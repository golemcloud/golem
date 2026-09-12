import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import {
  DurableStream,
  FetchError,
  IdempotentProducer,
} from "@durable-streams/client";

const [origin] = process.argv.slice(2);
const json = "application/json";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function session(method = "echo") {
  const agent = `${origin}/durable-stream-agents/${randomUUID()}`;
  const base = `${agent}/${method}/invocations/${randomUUID()}`;
  return { agent, base, slot: (name) => `${base}/streams/${name}` };
}

function handle(url, contentType = json) {
  return new DurableStream({ url, contentType, warnOnHttp: false });
}

async function create(url, contentType = json) {
  return DurableStream.create({ url, contentType, warnOnHttp: false });
}

async function request(url, status, init = {}, headers = {}) {
  const response = await fetch(url, {
    ...init,
    signal: AbortSignal.timeout(20_000),
  });
  assert.equal(
    response.status,
    status,
    `${init.method ?? "GET"} ${url}: ${await response.clone().text()}`,
  );
  for (const [name, value] of Object.entries(headers)) {
    assert.equal(response.headers.get(name), value, name);
  }
  return response;
}

async function closed(stream) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if ((await stream.head()).streamClosed) return;
    await sleep(20);
  }
  assert.fail(`stream did not close: ${stream.url}`);
}

async function readJson(stream) {
  const response = await stream.stream({ offset: "-1", live: false });
  const values = await response.json();
  assert.equal(response.streamClosed, true);
  assert.equal(response.headers.get("stream-closed"), "true");
  return values;
}

async function echoAndSink() {
  for (const [method, output, expected] of [
    ["echo", "output", ["first", "雪", "last"]],
    ["sink", "$result", ["first|雪|last"]],
  ]) {
    const s = session(method);
    const input = await create(s.slot("input"));
    await input.append(JSON.stringify("first"));
    await input.append(JSON.stringify("雪"));
    const terminal = await input.close({ body: JSON.stringify("last") });
    assert.equal((await input.head()).offset, terminal.finalOffset);
    const result = handle(s.slot(output));
    await closed(result);
    assert.deepEqual(await readJson(result), expected);
    assert.equal((await input.close()).finalOffset, terminal.finalOffset);
  }
  const s = session("echo-bytes");
  const input = await create(s.slot("input"), "application/octet-stream");
  await input.append(Uint8Array.of(0, 255, 128));
  await input.close({ body: Uint8Array.of(13, 10, 42) });
  const result = handle(s.slot("$result"), "application/octet-stream");
  await closed(result);
  const response = await result.stream({ offset: "-1", live: false });
  assert.deepEqual(
    new Uint8Array(await response.body()),
    Uint8Array.of(0, 255, 128, 13, 10, 42),
  );
  assert.equal(response.streamClosed, true);
}

async function idempotentProducer() {
  const s = session();
  const input = await create(s.slot("input"));
  const accepted = [];
  const producer = new IdempotentProducer(input, "reference-producer", {
    epoch: 7,
    maxInFlight: 1,
    lingerMs: 0,
    fetch: async (url, init) => {
      const response = await fetch(url, init);
      accepted.push({
        url,
        init,
        status: response.status,
        offset: response.headers.get("stream-next-offset"),
      });
      return response;
    },
  });
  producer.append(JSON.stringify("zero"));
  await producer.flush();
  producer.append(JSON.stringify("one"));
  await producer.flush();
  assert.deepEqual(
    accepted.map((r) => r.status),
    [200, 200],
  );
  const first = accepted[0];
  await request(first.url, 204, first.init, {
    "producer-seq": "1",
    "stream-next-offset": first.offset,
  });
  const post = (epoch, seq) => ({
    method: "POST",
    body: JSON.stringify(["must-not-appear"]),
    headers: {
      "content-type": json,
      "producer-id": "reference-producer",
      "producer-epoch": epoch,
      "producer-seq": seq,
    },
  });
  await request(s.slot("input"), 409, post("7", "3"), {
    "producer-expected-seq": "2",
    "producer-received-seq": "3",
  });
  await request(s.slot("input"), 403, post("6", "2"), {
    "producer-epoch": "7",
  });
  const successor = new IdempotentProducer(input, "reference-producer", {
    epoch: 8,
    maxInFlight: 1,
    lingerMs: 0,
  });
  successor.append(JSON.stringify("new-epoch"));
  await successor.flush();
  await request(s.slot("input"), 403, post("7", "2"), {
    "producer-epoch": "8",
  });
  const terminal = await successor.close(JSON.stringify("final"));
  await request(
    s.slot("input"),
    204,
    {
      ...post("8", "1"),
      headers: { ...post("8", "1").headers, "stream-closed": "true" },
    },
    { "stream-next-offset": terminal.finalOffset, "producer-seq": "1" },
  );
  await request(s.slot("input"), 409, post("8", "2"), {
    "stream-closed": "true",
  });
  const output = handle(s.slot("output"));
  await closed(output);
  assert.deepEqual(await readJson(output), [
    "zero",
    "one",
    "new-epoch",
    "final",
  ]);
}

async function offsetsAndClientCancellation() {
  const s = session();
  const input = await create(s.slot("input"));
  await input.append(JSON.stringify("before"));
  const history = await input.stream({ offset: "-1", live: false });
  assert.deepEqual(await history.json(), ["before"]);
  const now = await input.stream({ offset: "now", live: false });
  assert.deepEqual(await now.json(), []);
  assert.equal(now.offset, history.offset);
  assert.equal(now.upToDate, true);
  assert.equal(now.streamClosed, false);
  await input.append(JSON.stringify("after"));
  const resumed = await input.stream({ offset: now.offset, live: false });
  assert.deepEqual(await resumed.json(), ["after"]);
  assert.notEqual(resumed.offset, now.offset);

  // Cancelling a client subscription must not close the server's stream.
  let started;
  const subscribed = new Promise((resolve) => {
    started = resolve;
  });
  const listener = new DurableStream({
    url: s.slot("input"),
    contentType: json,
    warnOnHttp: false,
    fetch: async (url, init) => {
      const response = await fetch(url, init);
      if (
        new URL(url).searchParams.get("live") === "sse" &&
        response.status === 200
      )
        started();
      return response;
    },
  });
  const response = await listener.stream({
    offset: "now",
    live: "sse",
    signal: AbortSignal.timeout(20_000),
  });
  assert.equal(response.upToDate, true);
  const completion = response.closed.catch(() => {});
  const reader = response.jsonStream().getReader();
  const first = reader.read();
  await input.append(JSON.stringify("live"));
  await Promise.race([
    subscribed,
    completion.then(() => assert.fail("subscription ended before SSE opened")),
  ]);
  assert.deepEqual(await first, { value: "live", done: false });
  const pending = reader.read().catch(() => {});
  response.cancel();
  await Promise.all([completion, pending]);
  assert.equal((await input.head()).streamClosed, false);
  await input.close({ body: JSON.stringify("still-open") });
  assert.deepEqual(await readJson(input), [
    "before",
    "after",
    "live",
    "still-open",
  ]);
}

async function sessionCancellation() {
  const s = session();
  const input = await create(s.slot("input"));
  await input.append(JSON.stringify("retained"));
  const deadline = Date.now() + 20_000;
  for (;;) {
    const response = await handle(s.slot("output")).stream({
      offset: "-1",
      live: false,
    });
    if ((await response.json()).length > 0) break;
    assert.ok(
      Date.now() < deadline,
      "echo did not publish before cancellation",
    );
    await sleep(20);
  }
  await request(s.base, 204, { method: "DELETE" });
  await request(s.base, 204, { method: "DELETE" });
  await request(session().base, 404, { method: "DELETE" });
  for (const slot of ["input", "output"]) {
    const response = await handle(s.slot(slot)).stream({
      offset: "-1",
      live: false,
    });
    assert.deepEqual(await response.json(), ["retained"]);
    assert.equal(response.streamClosed, true);
    assert.equal(response.headers.get("stream-cancelled"), "true");
  }
  // The guest reaches its continuation after observing input EOF; DELETE is not an interrupt.
  const continuationDeadline = Date.now() + 20_000;
  for (;;) {
    const observations = await request(`${s.agent}/observations`, 200, {
      method: "PUT",
    });
    const values = await observations.json();
    if (values[4] === 1) {
      assert.deepEqual(values, [1, 1, 1, 0, 1, 0, 0]);
      break;
    }
    assert.ok(
      Date.now() < continuationDeadline,
      "guest did not continue after cancellation",
    );
    await sleep(20);
  }
  assert.equal(
    await (await request(`${s.agent}/mark/37`, 200, { method: "PUT" })).json(),
    37,
  );
}

async function tombstoneAndExclusions() {
  const s = session();
  const input = await create(s.slot("input"));
  await input.append(JSON.stringify("deleted"));
  await input.delete();
  await assert.rejects(
    input.delete(),
    (error) => error instanceof FetchError && error.status === 410,
  );
  for (const method of ["GET", "HEAD", "POST", "DELETE"]) {
    await request(s.slot("input"), 410, { method });
  }
  await request(s.slot("input"), 409, {
    method: "PUT",
    headers: { "content-type": json },
  });
  const next = session();
  await create(next.slot("input"));
  await handle(next.slot("input")).close();
  await request(session().slot("input"), 400, {
    method: "PUT",
    headers: { "content-type": json, "stream-ttl": "60" },
  });
  for (const method of ["GET", "PUT"]) {
    await request(s.slot("__ds"), 404, { method });
  }
}

async function readerCapAndRelease() {
  const s = session();
  const input = await create(s.slot("input"));
  const url = `${s.slot("input")}?offset=now&live=sse`;
  const controllers = [];
  const readers = [];
  async function subscribe() {
    const controller = new AbortController();
    controllers.push(controller);
    const response = await fetch(url, { signal: controller.signal });
    assert.equal(response.status, 200);
    const reader = response.body.getReader();
    readers.push(reader);
    assert.equal(
      (await reader.read()).done,
      false,
      "SSE must start with a control event",
    );
  }
  try {
    for (let i = 0; i < 16; i++) await subscribe();
    await request(url, 503, {}, { "retry-after": "1" });
    controllers[0].abort();
    await readers[0].cancel().catch(() => {});
    const deadline = Date.now() + 5_000;
    for (;;) {
      const controller = new AbortController();
      const response = await fetch(url, { signal: controller.signal });
      if (response.status === 200) {
        controllers.push(controller);
        readers.push(response.body.getReader());
        break;
      }
      assert.equal(response.status, 503);
      await response.arrayBuffer();
      assert.ok(
        Date.now() < deadline,
        "disconnect did not release reader admission",
      );
      await sleep(20);
    }
    await request(url, 503, {}, { "retry-after": "1" });
  } finally {
    for (const controller of controllers) controller.abort();
    await Promise.all(readers.map((reader) => reader.cancel().catch(() => {})));
    await input.close();
  }
}

for (const scenario of [
  echoAndSink,
  idempotentProducer,
  offsetsAndClientCancellation,
  sessionCancellation,
  tombstoneAndExclusions,
  readerCapAndRelease,
]) {
  console.log(`running ${scenario.name}`);
  await scenario();
  console.log(`passed ${scenario.name}`);
}
console.log("reference-client-export-protocol-ok");
