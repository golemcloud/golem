import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import http from "node:http";
import { test } from "node:test";

// Run against the disposable server provisioned by the CLI integration harness.
const base = process.argv[2];
assert.match(base ?? "", /^http:\/\/(localhost|127\.0\.0\.1):\d+$/);
const corpus = JSON.parse(readFileSync(new URL("../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json", import.meta.url), "utf8"));
const fixture = (id) => {
  const entry = corpus.cases.find((entry) => entry.id === id);
  assert.ok(entry, `unknown corpus ID ${id}`);
  return entry;
};
const request = (path, options = {}) => fetch(`${base}${path}`, { ...options, signal: AbortSignal.timeout(20_000) });

test("real Effect HttpApi, generated mounted OpenAPI, and host static files", async () => {
  const asset = await request("/static/value.txt");
  assert.equal(asset.status, 200);
  assert.equal(await asset.text(), "immutable effect asset");
  const item = await request("/catalog/items/abc");
  assert.equal(item.status, 200);
  assert.deepEqual(await item.json(), { id: "abc", count: 7 });
  const document = await (await request("/openapi.json")).json();
  assert.ok(document.paths["/catalog/items/{id}"].get);
  assert.equal(document.paths["/catalog/items/{id}"].get.responses["200"].content["application/json"].schema.properties.count.type, "integer");
});

test("normal agent clients and direct-module response hooks", async () => {
  const first = Number(await (await request("/web/counter")).text());
  assert.ok(Number.isInteger(first) && first > 0);
  assert.equal(Number(await (await request("/web/counter")).text()), first + 1);
  const hook = await request("/web/hook");
  assert.equal(hook.headers.get("x-hook"), "shared");
  assert.equal(await hook.text(), "hook");
});

test("canonical extension token and empty query survive the deployed boundary", async () => {
  const response = await request("/raw/%61?", { method: "CuStOm" });
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { method: "CuStOm", path: "/raw/%61", query: "" });
});

test("envelope-head-unpolled: public status, declared length, empty body", async () => {
  const { input, expect } = fixture("envelope-head-unpolled");
  const response = await request("/web/head", { method: input.method });
  assert.equal(response.status, expect.status);
  for (const [name, value] of expect.headers) assert.equal(response.headers.get(name), value);
  assert.equal(Buffer.from(await response.arrayBuffer()).toString("hex"), expect.body_hex);
});

test("early response does not wait for upload EOF", async () => {
  await new Promise((resolve, reject) => {
    const upload = http.request(`${base}/web/early`, { method: "POST" }, (response) => {
      assert.equal(response.statusCode, 202);
      const chunks = [];
      response.on("data", (chunk) => chunks.push(chunk));
      response.on("error", reject);
      response.on("end", () => {
        try { assert.equal(Buffer.concat(chunks).toString(), "early"); resolve(); } catch (error) { reject(error); }
        upload.destroy();
      });
    });
    upload.on("error", reject);
    upload.setTimeout(20_000, () => upload.destroy(new Error("early response waited for EOF")));
    upload.write("partial upload");
  });
});

test("incremental echo and envelope-cookie-order through the real router", async () => {
  const { expect } = fixture("envelope-cookie-order");
  await new Promise((resolve, reject) => {
    let sentSecond = false;
    const chunks = [];
    const upload = http.request(`${base}/web/echo`, { method: "POST" }, (response) => {
      try {
        assert.equal(response.statusCode, expect.status);
        assert.deepEqual(response.headers["set-cookie"], expect.headers.filter(([name]) => name === "set-cookie").map(([, value]) => value));
        assert.equal(response.headers["x-public"], "between");
      } catch (error) { reject(error); upload.destroy(); return; }
      response.on("error", reject);
      response.on("data", (chunk) => {
        chunks.push(chunk);
        if (!sentSecond && Buffer.concat(chunks).length >= 5) {
          sentSecond = true;
          upload.end("second");
        }
      });
      response.on("end", () => {
        try { assert.ok(sentSecond); assert.equal(Buffer.concat(chunks).toString(), "firstsecond"); resolve(); } catch (error) { reject(error); }
      });
    });
    upload.on("error", reject);
    upload.setTimeout(20_000, () => upload.destroy(new Error("response buffered the upload")));
    upload.write("first");
  });
});

test("failures before the head and after committed headers are not successful responses", async () => {
  const early = await request("/web/before-head");
  assert.ok(early.status >= 500);
  const late = await request("/web/after-head");
  assert.equal(late.status, 200);
  await assert.rejects(late.arrayBuffer());
});
