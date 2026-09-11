use crate::app::{TestContext, cmd, flag};
use crate::{Tracing, workspace_path};
use golem_cli::fs;
use golem_cli::versions;
use indoc::formatdoc;
use std::process::Stdio;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::process::Command;

inherit_test_dep!(Tracing);

async fn run(command: &mut Command, timeout: Duration) -> std::process::Output {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    tokio::time::timeout(timeout, command.output())
        .await
        .expect("subprocess timed out")
        .expect("failed to start subprocess")
}

fn assert_success(output: std::process::Output, command: &str) {
    assert!(
        output.status.success(),
        "{command} failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[timeout("10 minutes")]
async fn reference_client_reads_json_and_bytes_in_all_ds3_modes() {
    let mut ctx = TestContext::new();
    let fixture = workspace_path().join("test-components/golem_it_agent_sdk_rust_release.wasm");
    assert!(
        fixture.exists(),
        "missing durable stream fixture {}; build the agent-sdk-rust release test component first",
        fixture.display()
    );

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {manifest_version}

            app: durable-stream-reference-client

            componentTemplates:
              prebuilt:
                componentWasm: "{fixture}"

            components:
              golem-it:agent-sdk-rust:
                templates: prebuilt

            httpApi:
              deployments:
                local:
                - domain: localhost:9006
                  agents:
                    DurableStreamAgent: {{}}

            environments:
              local:
                server: local
        "#, manifest_version = versions::sdk::MANIFEST, fixture = fixture.display()},
    )
    .unwrap();

    ctx.start_server().await;
    let deployed = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(deployed.success_or_dump());

    let driver_dir = ctx.cwd_path_join("reference-client");
    fs::create_dir_all(&driver_dir).unwrap();
    fs::write_str(
        driver_dir.join("package.json"),
        r#"{
  "private": true,
  "type": "module",
  "dependencies": {
    "@durable-streams/client": "0.2.7"
  }
}
"#,
    )
    .unwrap();
    fs::write_str(driver_dir.join("driver.mjs"), DRIVER).unwrap();

    let mut npm = Command::new("npm");
    npm.args(["install", "--ignore-scripts", "--no-audit", "--no-fund"])
        .current_dir(&driver_dir);
    assert_success(run(&mut npm, Duration::from_secs(120)).await, "npm install");

    let origin = format!("http://localhost:{}", ctx.custom_request_port());
    let mut node = Command::new("node");
    node.arg("driver.mjs").arg(origin).current_dir(&driver_dir);
    assert_success(
        run(&mut node, Duration::from_secs(180)).await,
        "reference client driver",
    );
}

const DRIVER: &str = r#"
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { DurableStream } from "@durable-streams/client";

const [origin] = process.argv.slice(2);
const liveRequests = [];
const tracedFetch = (input, init) => {
  const url = new URL(input instanceof Request ? input.url : input);
  if (url.searchParams.has("live")) liveRequests.push(url.searchParams.get("live"));
  return fetch(input, init);
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
