---
name: golem-add-llm-effect
description: "Adding LLM and AI capabilities to an Effect-based Golem agent. Use when an @golemcloud/effect-golem project needs chat completions, embeddings, tool calling, or an AI provider integration."
---

# Adding LLM and AI Capabilities to an Effect Golem Agent

## Current SDK Limitation

`@golemcloud/effect-golem` 1.5.0 does not provide an LLM host API or a supported way for component
code to make an outgoing HTTP request. An Effect agent therefore cannot call OpenAI-compatible
chat completions, Anthropic messages, embeddings, or another remote AI API with this SDK version.

This is a component capability limitation, not just a missing convenience wrapper:

- The SDK's `agent-guest` world in `wit/main.wit` imports neither a Golem LLM interface nor
  `wasi:http/outgoing-handler`.
- The prebuilt QuickJS component does not install a global `fetch` implementation.
- The package exports no outbound `HttpClient` or LLM service. Its `Http` namespace only declares
  incoming agent routes.
- The exported `Websocket` client is a specialized host capability, not an HTTP transport for REST
  APIs such as OpenAI's chat completions endpoint.
- `@effect/platform` is not embedded, and the SDK supplies no WASI HTTP layer for it.

See the pinned SDK sources:

- [`wit/main.wit`](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/wit/main.wit)
- [`src/index.ts`](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/index.ts)
- [`src/host/HostLive.ts`](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/host/HostLive.ts)
- [`scripts/generate-agent-template.mjs`](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/scripts/generate-agent-template.mjs)

## Handling an LLM Request

1. Check the installed `@golemcloud/effect-golem` version and its authoritative WIT world and
   exports. Do not assume plain TypeScript SDK capabilities also exist in the Effect SDK.
2. If it has the limitation above, explain that the requested provider call cannot be implemented
   in this component. Keep independent supported edits buildable, but do not add an LLM method that
   only traps, returns `undefined`, returns a canned response, or pretends to call the provider.
3. Offer one of these supported alternatives:
   - use a Golem language SDK whose component world supports outgoing HTTP;
   - perform the LLM call in an external service or orchestrator and invoke the Effect agent for
     the durable agent work;
   - upgrade to a later Effect SDK release only when its source or documentation explicitly shows
     the required host LLM or outgoing HTTP capability.
4. If a later SDK adds the capability, inspect its exact exports and examples before writing code.
   Keep the agent contract in `defineAgent`/`method`, declare all values with Effect `Schema`, and
   implement the handler as an `Effect` rather than a plain `async` function.

## Do Not Use Apparent Workarounds

These approaches do not work with the current Effect component:

```typescript
// No global fetch implementation or wasi:http capability is present.
Effect.tryPromise(() => fetch(url, request));

// Installing a client cannot add a component-model host capability.
import OpenAI from "openai";

// @effect/platform has no HTTP transport layer in this runtime.
import { HttpClient } from "@effect/platform";

// Rollup externalization does not add this missing import to the component world.
import * as OutgoingHandler from "wasi:http/outgoing-handler@0.2.3";
```

Do not invent helpers such as `Llm.chat`, `GolemLlm`, or `EffectGolemHttpClient`; none are exported.
Do not import `@golemcloud/golem-ts-sdk` into an Effect component or copy its `fetch` examples. An
npm package can bundle successfully and still fail at runtime when the underlying host capability
is absent. Likewise, a successful build or deployment does not prove that an unavailable outbound
host call will work at invocation time.

## Capability Checklist for a Future SDK

Before implementing an LLM provider call, verify all of the following in the installed Effect SDK:

- its WIT world imports a host LLM interface or `wasi:http/outgoing-handler`;
- generated bindings or a documented public client expose that import to application code;
- the prebuilt QuickJS component supplies the required JavaScript or Effect transport;
- the provider client supports that transport without native Node addons or unsupported globals;
- secrets remain redacted and are only unwrapped at the call boundary.

If any item is missing, stop and report the precise missing capability instead of guessing an API.
