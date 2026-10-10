import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";

const base = process.argv[2];
assert.match(base ?? "", /^http:\/\/(localhost|127\.0\.0\.1):\d+$/);
const metrics = process.argv[3];
assert.match(metrics ?? "", /^http:\/\/(localhost|127\.0\.0\.1):\d+\/metrics$/);
const request = (path, options = {}) =>
  fetch(`${base}${path}`, { ...options, signal: AbortSignal.timeout(20_000) });

// Run first so failure probes cannot contribute delayed cancellation increments.
test("disconnect open uploads before and after headers, then serve another invocation", async () => {
  const cancellations = async () => {
    const snapshot = await (
      await fetch(metrics, { signal: AbortSignal.timeout(20_000) })
    ).text();
    return Number(
      snapshot.match(
        /^http_session_explicit_cancellations_total (\d+)$/m,
      )?.[1] ?? 0,
    );
  };
  const entered = http.createServer();
  entered.listen(0, "127.0.0.1");
  await once(entered, "listening");
  try {
    for (const phase of ["before", "after"]) {
      const previous = await cancellations();
      await new Promise((resolve, reject) => {
        let deliberatelyDestroyed = false;
        const upload = http.request(`${base}/checks/disconnect-${phase}`, {
          method: "POST",
          headers: {
            "x-entered": `http://127.0.0.1:${entered.address().port}/entered`,
          },
        });
        upload.setTimeout(20_000, () =>
          upload.destroy(new Error("disconnect stalled")),
        );
        upload.on("error", (error) => {
          if (!deliberatelyDestroyed || error.code !== "ECONNRESET")
            reject(error);
        });
        upload.on("close", () => {
          if (deliberatelyDestroyed) resolve();
          else reject(new Error(`premature close ${phase} headers`));
        });
        upload.on("response", (response) => {
          if (phase === "before" || response.statusCode !== 200) {
            reject(
              new Error(
                `unexpected response ${phase} headers: ${response.statusCode}`,
              ),
            );
            upload.destroy();
            return;
          }
          const chunks = [];
          response.on("data", (chunk) => {
            chunks.push(chunk);
            if (Buffer.concat(chunks).length < 7) return;
            try {
              assert.equal(Buffer.concat(chunks).toString(), "partial");
            } catch (error) {
              reject(error);
              upload.destroy();
              return;
            }
            deliberatelyDestroyed = true;
            upload.destroy();
          });
          response.on("error", (error) => {
            if (!deliberatelyDestroyed) reject(error);
          });
        });
        if (phase === "before")
          entered.once("request", (_req, res) => {
            res.end();
            // The guest has entered the handler; cancel without committing headers.
            deliberatelyDestroyed = true;
            upload.destroy();
          });
        upload.write("partial"); // Keep upload EOF withheld in both phases.
      });
      // Client close is not proof of host cleanup. Observe the host's terminal cancellation.
      const deadline = Date.now() + 20_000;
      while ((await cancellations()) <= previous && Date.now() < deadline)
        await delay(100);
      assert.ok(
        (await cancellations()) > previous,
        `host did not cancel ${phase} headers`,
      );
      assert.equal(await (await request("/")).text(), "Hello from Golem");
    }
  } finally {
    await new Promise((resolve, reject) =>
      entered.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("router-only exact example and shared import forms", async () => {
  assert.equal(await (await request("/")).text(), "Hello from Golem");
  assert.equal(await (await request("/checks/imports")).text(), "true");
});

test("headers before input and response bytes before upload EOF", async () => {
  await new Promise((resolve, reject) => {
    let sentSecond = false;
    const chunks = [];
    const upload = http.request(
      `${base}/checks/echo`,
      { method: "POST" },
      (response) => {
        try {
          assert.equal(response.statusCode, 200);
          assert.deepEqual(response.headers["set-cookie"], [
            "a=first",
            "a=second",
          ]);
          // No body has been sent yet: receipt of headers unlocks the first chunk.
          upload.write("first");
        } catch (error) {
          upload.destroy();
          reject(error);
          return;
        }
        response.on("error", reject);
        response.on("data", (chunk) => {
          chunks.push(chunk);
          if (!sentSecond && Buffer.concat(chunks).length >= 5) {
            sentSecond = true;
            upload.end("second");
          }
        });
        response.on("end", () => {
          try {
            assert.ok(sentSecond);
            assert.equal(Buffer.concat(chunks).toString(), "firstsecond");
            resolve();
          } catch (error) {
            reject(error);
          }
        });
      },
    );
    upload.on("error", reject);
    upload.setTimeout(20_000, () =>
      upload.destroy(new Error("streaming handshake stalled")),
    );
    upload.flushHeaders();
  });
});

test("HEAD, status suppression and zero-length disposal before end", async () => {
  for (const options of [
    { method: "HEAD" },
    ...[204, 205, 304].map((status) => ({
      headers: { "x-status": String(status) },
    })),
    { headers: { "x-zero": "yes" } },
  ]) {
    const response = await request("/checks/bodyless", options);
    assert.equal(response.status, Number(options.headers?.["x-status"] ?? 200));
    assert.equal((await response.arrayBuffer()).byteLength, 0);
  }
});

test("outgoing HTTP remains the original runtime client", async () => {
  const peer = http.createServer((req, res) => {
    assert.equal(req.url, "/client");
    res.end("original-client");
  });
  peer.listen(0, "127.0.0.1");
  await once(peer, "listening");
  try {
    const url = `http://127.0.0.1:${peer.address().port}/client`;
    assert.equal(
      await (
        await request(`/checks/outgoing?url=${encodeURIComponent(url)}`)
      ).text(),
      "original-client",
    );
  } finally {
    await new Promise((resolve, reject) =>
      peer.close((error) => (error ? reject(error) : resolve())),
    );
  }
});

test("errors before and after header commitment", async () => {
  assert.ok((await request("/checks/before-head")).status >= 500);
  await new Promise((resolve, reject) => {
    const upload = http.request(
      `${base}/checks/after-head`,
      { method: "POST" },
      (response) => {
        try {
          assert.equal(response.statusCode, 200);
        } catch (error) {
          upload.destroy();
          reject(error);
          return;
        }
        const failedBody = (async () => {
          for await (const _chunk of response) {
          }
        })();
        void assert.rejects(failedBody).then(resolve, reject);
        // Only cause the guest failure after headers have reached this caller.
        upload.end("fail now");
      },
    );
    upload.on("error", reject);
    upload.setTimeout(20_000, () =>
      upload.destroy(new Error("failure handshake stalled")),
    );
    upload.flushHeaders();
  });
});
