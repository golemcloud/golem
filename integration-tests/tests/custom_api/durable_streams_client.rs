use crate::custom_api::http_test_context::HttpTestContext;
use golem_test_framework::components::durable_streams_client;
use test_r::{define_matrix_dimension, inherit_test_dep, test, timeout};

inherit_test_dep!(HttpTestContext);
inherit_test_dep!(
    #[tagged_as("postgres")]
    HttpTestContext
);
inherit_test_dep!(
    #[tagged_as("sqlite")]
    HttpTestContext
);
define_matrix_dimension!(db: HttpTestContext -> "postgres", "sqlite");

#[test]
#[timeout("10 minutes")]
async fn reference_client_reads_json_and_bytes_in_all_ds3_modes(
    #[dimension(db)] context: &HttpTestContext,
) {
    run_reference_client(context, DRIVER).await;
}

#[test]
#[timeout("10 minutes")]
async fn reference_client_export_protocol_compatibility(
    #[dimension(db)] context: &HttpTestContext,
) {
    run_reference_client(context, include_str!("durable_streams_client.mjs")).await;
}

async fn run_reference_client(context: &HttpTestContext, driver: &str) {
    durable_streams_client::run(
        driver,
        context.base_url.as_str().trim_end_matches('/'),
        context.host_header.to_str().unwrap(),
    )
    .await
    .unwrap();
}

const DRIVER: &str = r#"
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { DurableStream } from "@durable-streams/client";

const [origin] = process.argv.slice(2);
const liveRequests = [];
const tracedFetch = async (input, init) => {
  const url = new URL(input instanceof Request ? input.url : input);
  if (url.searchParams.has("live")) liveRequests.push(url.searchParams.get("live"));
  const response = await fetch(input, init);
  if (url.pathname.includes("/bytes/") && url.searchParams.get("live") === "sse") {
    assert.equal(response.headers.get("stream-sse-data-encoding"), "base64");
  }
  return response;
};
const expectedJson = count => Array.from({ length: count }, (_, i) => `message-${String(i).padStart(4, "0")}`);
const expectedBytes = count => Uint8Array.from({ length: count }, (_, i) => i % 251);

function url(kind, count, delayMs, session = randomUUID()) {
  return `${origin}/durable-stream-agents/${randomUUID()}/${kind}/${count}/invocations/${session}/streams/$result?delay_ms=${delayMs}`;
}

async function create(kind, count, delayMs) {
  const streamUrl = url(kind, count, delayMs);
  const handle = await DurableStream.create({
    url: streamUrl, warnOnHttp: false, fetch: tracedFetch,
    contentType: kind === "json" ? "application/json" : "application/octet-stream",
  });
  const metadata = await handle.head();
  assert.equal(metadata.exists, true);
  assert.equal(metadata.contentType, kind === "json" ? "application/json" : "application/octet-stream");
  return handle;
}

async function waitForClosed(handle) {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if ((await handle.head()).streamClosed) return;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert.fail("stream did not close");
}

async function catchUpJson() {
  const count = 17;
  const handle = await create("json", count, 0);
  await waitForClosed(handle);
  const items = [];
  let offset = "-1";
  for (;;) {
    const response = await handle.stream({ offset, live: false });
    items.push(...await response.json());
    if (response.upToDate) break;
    assert.notEqual(response.offset, offset, "catch-up cursor must advance");
    offset = response.offset;
  }
  assert.deepEqual(items, expectedJson(count));
}

async function catchUpBytes() {
  const count = 521;
  const handle = await create("bytes", count, 0);
  await waitForClosed(handle);
  const chunks = [];
  let offset = "-1";
  for (;;) {
    const response = await handle.stream({ offset, live: false });
    chunks.push(new Uint8Array(await response.body()));
    if (response.upToDate) break;
    assert.notEqual(response.offset, offset, "catch-up cursor must advance");
    offset = response.offset;
  }
  const actual = Uint8Array.from(chunks.flatMap(chunk => [...chunk]));
  assert.deepEqual(actual, expectedBytes(count));
}

async function liveJson(mode) {
  const count = 6;
  liveRequests.length = 0;
  const handle = await create("json", count, 200);
  assert.equal((await handle.head()).streamClosed, false);
  const response = await handle.stream({ offset: "-1", live: mode });
  const values = [];
  const reader = response.jsonStream().getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    values.push(value);
  }
  assert.deepEqual(values, expectedJson(count));
  assert.equal(response.streamClosed, true);
  await response.closed;
  assert.ok(liveRequests.includes(mode), `${mode} JSON read never used the live transport`);
}

async function liveBytes(mode) {
  const count = 96;
  liveRequests.length = 0;
  const handle = await create("bytes", count, 20);
  assert.equal((await handle.head()).streamClosed, false);
  const response = await handle.stream({ offset: "-1", live: mode });
  const values = [];
  const reader = response.bodyStream().getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    values.push(...value);
  }
  assert.deepEqual(Uint8Array.from(values), expectedBytes(count));
  assert.equal(response.streamClosed, true);
  await response.closed;
  assert.ok(liveRequests.includes(mode), `${mode} bytes read never used the live transport`);
}

await catchUpJson();
await catchUpBytes();
await liveJson("long-poll");
await liveBytes("long-poll");
await liveJson("sse");
await liveBytes("sse");
console.log("reference-client-ds3-ok");
"#;
