/**
 * Webhook case — exercises the full `Webhook` round-trip.
 *
 *   1. `prime` mints a host promise + signed URL (recorded in the
 *      oplog as `CALL golem::api::create_promise` then
 *      `CALL golem::agent::create_webhook`).
 *   2. `waitForEvent` durably suspends on the promise; the harness
 *      forks the CLI invoke so we can POST to the URL while the
 *      forked invocation is still parked.
 *   3. `fetch(url, { method: "POST", body: <payload> })` completes the
 *      promise atomically with the request body bytes; the host
 *      returns 204 No Content.
 *   4. The forked invoke's stdout contains the round-tripped body.
 */
import { Duration, Effect, Fiber } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const name = `wh-${session.stamp}`
  const r = `WebhookAgent("${name}")`

  // Allocate the URL. The host-built URL has shape
  //   https://<domain>/<webhooksPrefix>/<webhookSuffix>/<base64url(AgentWebhookId)>
  // — for the integration deployment that's
  //   http://effect-golem.localhost:9006/webhooks/inbox/<…>
  // The CLI also prints a "Selected app: ... (http://localhost:9881/)"
  // header, so we narrow the URL match to the webhook prefix +
  // suffix path.
  const primed = yield* liftCliError(cli.invoke(r, "prime"))
  const urlMatch = primed.stdout.match(/https?:\/\/[^\s"',)]+\/webhooks\/inbox\/[^\s"',)]+/)
  if (!urlMatch) {
    return yield* Effect.fail(
      new TestFailure({
        testName: session.currentTest,
        message: "could not parse webhook URL out of `prime` stdout",
        diagnostic: primed.stdout,
      }),
    )
  }
  const url = urlMatch[0] as string
  yield* expectMatch(url, /\/webhooks\/inbox\//, "minted URL contains the webhookSuffix /inbox/")

  // Sanity: isPrimed → true.
  const isPrimed = yield* liftCliError(cli.invoke(r, "isPrimed"))
  yield* expectMatch(isPrimed.stdout, /true/, "WebhookAgent.isPrimed returns true after prime")

  // Fork the durably-suspending wait. Pre-lift the GolemCliError
  // into TestFailure so Fiber.join below produces a homogeneous
  // error channel.
  const waitFiber = yield* Effect.forkChild(liftCliError(cli.invoke(r, "waitForEvent")))

  // Give the host a moment to enter the SUSPEND state, then POST.
  yield* Effect.sleep(Duration.seconds(2))

  const payload = `hello-from-${session.stamp}`
  const post = yield* Effect.tryPromise({
    try: () =>
      fetch(url, {
        method: "POST",
        headers: { "content-type": "text/plain" },
        body: payload,
      }),
    catch: (cause) =>
      new TestFailure({
        testName: session.currentTest,
        message: `POST to webhook URL ${url} threw`,
        diagnostic: String(cause),
      }),
  })
  if (post.status !== 200 && post.status !== 204) {
    return yield* Effect.fail(
      new TestFailure({
        testName: session.currentTest,
        message: `POST to webhook URL returned unexpected status ${post.status}`,
        diagnostic: `url=${url}`,
      }),
    )
  }

  // Wait for the durably-suspended invocation to wake up.
  const waitResult = yield* Fiber.join(waitFiber).pipe(
    Effect.timeoutOrElse({
      duration: Duration.seconds(60),
      orElse: () =>
        Effect.fail(
          new TestFailure({
            testName: session.currentTest,
            message: "waitForEvent did not return within 60s after POST",
          }),
        ),
    }),
  )
  yield* expectMatch(
    waitResult.stdout,
    new RegExp(payload),
    "waitForEvent stdout contains the round-tripped POST body",
  )

  // Oplog confirms the wire-level layout.
  const oplog = yield* liftCliError(cli.oplog(r))
  yield* expectMatch(oplog.stdout, /create_promise/i, "oplog has create_promise call")
  yield* expectMatch(oplog.stdout, /create_webhook/i, "oplog has create_webhook call")
})

export const case_ = defineCase(
  "webhook",
  "WebhookAgent: prime → POST → durably-suspended waitForEvent round-trip",
  run,
)
