---
name: golem-add-llm-effect
description: "Adding provider-backed LLM text generation to an Effect-based Golem agent with effect/unstable/ai and an Effect AI provider. Use when an @golemcloud/effect-golem project needs a non-streaming language-model request or OpenAI-compatible provider integration."
---

# Adding LLM Capabilities to an Effect Golem Agent

Use the provider-independent `LanguageModel` service from `effect/unstable/ai`, an exactly
version-aligned Effect AI provider package, and `FetchHttpClient.layer` from
`effect/unstable/http`. For OpenAI or an OpenAI-compatible endpoint, use `@effect/ai-openai`.

```diagram
┌────────────────────┐    provides     ┌───────────────────────────┐
│ Application method │◀───────────────│ LanguageModel service     │
└────────────────────┘                 └─────────────┬─────────────┘
                                                    │ OpenAiLanguageModel.layer
                                      ┌─────────────▼─────────────┐
                                      │ OpenAiClient              │
                                      └─────────────┬─────────────┘
                                                    │ FetchHttpClient.layer
                                      ┌─────────────▼─────────────┐
                                      │ WASI-backed global fetch  │
                                      └───────────────────────────┘
```

The currently validated contract is non-streaming `generateText`. Do not add structured
generation, streaming, embeddings, or tool calling unless the task explicitly requires it and the
installed pinned versions are verified for that capability.

## Install the Matching Provider Version

Effect packages use matching versions. Inspect the exact installed `effect` version, then install
the same `@effect/ai-openai` version. For the current template:

```shell
npm install --save-exact @effect/ai-openai@4.0.0-beta.98
```

Do not add the provider to `@golemcloud/effect-golem` or the common project template. Install it only
in applications that select OpenAI. A version mismatch can create incompatible Effect service
identities or types.

## Imports

```typescript
import { OpenAiClient, OpenAiLanguageModel } from "@effect/ai-openai";
import { Effect, Layer, Schema } from "effect";
import { LanguageModel } from "effect/unstable/ai";
import { FetchHttpClient } from "effect/unstable/http";
import { defineAgent, defineConfig, method } from "@golemcloud/effect-golem";
```

Application methods depend on `LanguageModel.LanguageModel`, not provider-specific response types.
Provider modules appear only in layer construction.

## Store Provider Configuration and Secrets

Declare the API key as a typed redacted secret. Keep the provider URL and model configurable so
development and tests can target an OpenAI-compatible service:

```typescript
export class LlmConfig extends defineConfig("Llm.Config", {
  apiKey: Schema.Redacted(Schema.String),
  apiUrl: Schema.String,
  model: Schema.String,
}) {}
```

Attach `config: LlmConfig` to the agent. Supply `apiKey` through Golem secrets and `apiUrl`/`model`
through ordinary config in `golem.yaml`. The OpenAI client's option is named **`apiUrl`**, and an
OpenAI-compatible base normally includes its `/v1` prefix.

Never log, return, snapshot, or interpolate the secret. `cfg.apiKey.get` already returns the
`Redacted<string>` required by `OpenAiClient.layer`; do not unwrap it in application code.

## Compose the Layers

The provider client requires the canonical `HttpClient` service, and the language-model layer
requires the provider client:

```typescript
const OpenAiClientLive = OpenAiClient.layer({
  apiKey,
  apiUrl,
}).pipe(Layer.provide(FetchHttpClient.layer));

const LanguageModelLive = OpenAiLanguageModel.layer({ model }).pipe(
  Layer.provide(OpenAiClientLive),
);
```

This composition provides the same `LanguageModel.LanguageModel` tag imported by application code
without bundling another Effect runtime.

## Generate Text

`LanguageModel.generateText` accepts a plain string prompt and returns a normalized,
provider-independent response. Read `response.text`:

```typescript
const answer = (question: string) =>
  LanguageModel.generateText({
    prompt: question,
    toolChoice: "none",
  }).pipe(
    Effect.map((response) => response.text),
    Effect.provide(LanguageModelLive),
  );
```

Provider status, transport, and response-decoding failures remain typed `AiError` failures. Map
them to a method's declared `error` schema when callers should handle them. Use `Effect.orDie` only
when the method intentionally exposes no recoverable provider failure.

## Complete Effect-Golem Agent

This example resolves typed Golem config when constructing the agent instance, builds the provider
layers from the resolved values, and keeps the handler provider-independent:

```typescript
import { OpenAiClient, OpenAiLanguageModel } from "@effect/ai-openai";
import { Effect, Layer, Schema } from "effect";
import { LanguageModel } from "effect/unstable/ai";
import { FetchHttpClient } from "effect/unstable/http";
import { defineAgent, defineConfig, method } from "@golemcloud/effect-golem";

export class LlmConfig extends defineConfig("Llm.Config", {
  apiKey: Schema.Redacted(Schema.String),
  apiUrl: Schema.String,
  model: Schema.String,
}) {}

export const LlmAgent = defineAgent({
  name: "LlmAgent",
  mode: "durable",
  constructorParams: { name: Schema.String },
  config: LlmConfig,
  methods: {
    ask: method({
      params: { question: Schema.String },
      success: Schema.String,
    }),
  },
}).implement(() =>
  Effect.gen(function* () {
    const cfg = yield* LlmConfig;
    const apiKey = yield* cfg.apiKey.get;
    const apiUrl = yield* cfg.apiUrl;
    const model = yield* cfg.model;

    const OpenAiClientLive = OpenAiClient.layer({ apiKey, apiUrl }).pipe(
      Layer.provide(FetchHttpClient.layer),
    );
    const LanguageModelLive = OpenAiLanguageModel.layer({ model }).pipe(
      Layer.provide(OpenAiClientLive),
    );

    const ask = ({ question }: { readonly question: string }) =>
      LanguageModel.generateText({
        prompt: question,
        toolChoice: "none",
      }).pipe(
        Effect.map((response) => response.text),
        Effect.provide(LanguageModelLive),
        Effect.orDie,
      );

    return { ask };
  }),
);
```

Import the implementation module from `src/main.ts` using its emitted `.js` suffix so the
top-level registration runs.

## Durability and Provider Side Effects

`FetchHttpClient.layer` uses the same WASI-backed outgoing HTTP path as other Effect HTTP requests.
Golem records the call for durable replay of the same invocation. Replay is not cross-invocation
deduplication: invoking `ask` twice can send two provider requests and incur two charges.

Do not log request headers or provider configuration. Public oplog conversion redacts recognized
credential-bearing HTTP headers, but application code must still keep secrets out of logs, return
values, error strings, snapshots, and diagnostics.

When testing against a mock provider, verify the request URL, bearer authorization, model, prompt,
and returned text. Also verify that replay does not unexpectedly resend a recorded request.

## Do Not Substitute Other Capabilities

- Do not call the provider with `Effect.tryPromise(globalThis.fetch)`; use `LanguageModel` and the
  provider layer.
- Do not return canned or stubbed text while claiming a provider request occurred.
- Do not use typed Golem RPC as a substitute for a requested external provider call.
- Do not install the Node `openai` package or another transport that bypasses Effect's canonical
  `HttpClient` service.
- Do not invent `Llm.chat`, `GolemLlm`, or `EffectGolemHttpClient` helpers.
- Do not import `@golemcloud/golem-ts-sdk` into an Effect component.

## Key Constraints

- Pin `@effect/ai-openai` exactly to the installed `effect` version.
- Import `LanguageModel` from `effect/unstable/ai`.
- Provide `FetchHttpClient.layer` to `OpenAiClient.layer`.
- Provide the client layer to `OpenAiLanguageModel.layer`.
- Keep application logic provider-independent and return `response.text`.
- Keep API keys as `Redacted<string>` from typed Golem secrets.
- Start with non-streaming `generateText` and `toolChoice: "none"`.
- Preserve typed provider errors unless the method intentionally converts them to defects.

## Authoritative API Sources

- [Effect v4 `LanguageModel`](https://github.com/Effect-TS/effect-smol/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/effect/src/unstable/ai/LanguageModel.ts)
- [Effect v4 fetch client](https://github.com/Effect-TS/effect-smol/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/effect/src/unstable/http/FetchHttpClient.ts)
- [`@effect/ai-openai` client](https://github.com/Effect-TS/effect-smol/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/ai/openai/src/OpenAiClient.ts)
- [`@effect/ai-openai` language model](https://github.com/Effect-TS/effect-smol/blob/3e4abbcb0d0e9a5e82b6b88c7ef7ab69900105ec/packages/ai/openai/src/OpenAiLanguageModel.ts)
- [Effect-Golem typed config](https://github.com/golemcloud/effect-golem/blob/HEAD/src/Config.ts)
