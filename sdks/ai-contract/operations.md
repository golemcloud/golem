# Operations, call forms and their types

**Layer:** binding: which operations each capability has, the bound and exact call forms, and the
rules written in prose (the model call and the logical exchange, pinning, event order, fallback, the
`estimateTokens` algorithm, one vector per input, rerank order). Reference model: the input, option,
result and event records.

Every SDK offers the operations below on the five capability types. The records on this page are
the reference model ([README.md § Layers](README.md#layers)): each SDK exposes the same information
and choices, with its own names, shapes and option styles, and keeps the behaviour written in prose
here. Names fixed by the spec (`run`, `runWith`) follow each SDK's casing (`runWith` or `run_with`).
Each operation also takes the invocation's cancellation signal in the SDK's form
([interfaces.md](interfaces.md#cancellation)); it is left out of the signatures below.

All operations are asynchronous in the SDK's idiom (a `Promise`, a `Future`, an `Effect`, an async
function or a function that takes `ctx`). Every failure is an `AiError` ([errors.md](errors.md)),
except cancellation, which ends the operation in the SDK's cancellation form.

## Operations by capability

| Capability | Operation | In | Out |
| -- | -- | -- | -- |
| `Llm` | `run` (bound form) | `input`, `LlmOptions` | `LlmResult` |
| `Llm` | `runWith` (exact form) | `input`, `LlmOptions` | `LlmResult` |
| `Llm` | `stream` \[PROP\] (bound form) | `input`, `LlmOptions` | `stream<LlmEvent>` |
| `Llm` | `streamWith` \[PROP\] (exact form) | `input`, `LlmOptions` | `stream<LlmEvent>` |
| `Llm` | `estimateTokens` \[PROP\] | `string` | `u64`, local, never fails |
| `Llm` | `countTokens` \[PROP\] | `input`, `LlmOptions` | `u64` |
| `Transcription` | `transcribe` \[PROP\] | `Audio`, `TranscribeOptions` | `Transcript` |
| `Transcription` | `transcribeStream` \[PROP\] | `stream<AudioChunk>`, `TranscribeOptions` | `stream<TranscriptEvent>` |
| `Speech` | `speak` \[PROP\] | `string`, `SpeakOptions` | `SpeechResult` |
| `Speech` | `speakStream` \[PROP\] | `string` or `stream<string>`, `SpeakOptions` | `stream<SpeechEvent>` |
| `ImageModel` | `generate` \[PROP\] | `ImageRequest` | `ImageResult` |
| `Embedding` | `embed` \[PROP\] | `list<EmbedInput>`, `EmbedOptions` | `EmbedResult` |
| `Embedding` | `rerank` \[PROP\] | `RerankRequest` | `RerankResult` |

`run` and `stream` offer the bound toolset plus the local tools passed; `runWith` and `streamWith`
offer only the local tools passed, or none (Rule 6; [tool-loop.md](tool-loop.md)). With no tools,
`runWith` is plain generation, and that is a valid call.

A call the serving provider cannot make fails with `AiError` kind `unsupported`; it never hangs or
degrades silently (Rule 4). [providers.md](providers.md#matrix) lists which calls each provider can
make.

## The model call and the logical exchange

A **model call** is one call of one operation above, other than `estimateTokens`. A model call is
the unit of Rule 3 (pinning), Rule 9 (never sent again once completed) and Rule 12 (usage counted
once). `countTokens` is a model call for Rules 3 and 9 but not for Rule 12: it reserves and commits
nothing (plan D7).

A model call of `Llm` is a **logical exchange**: one call as the caller sees it. Below the caller
it may involve several **provider requests**:

- the steps of a tool loop, one provider request each;
- retries of a provider request under the ambient retry rules (`golem.retry`, 1.4 Rule 21) or a
  `RetryPolicy` applied in code;
- fallback members tried in order ([Fallback](#fallback));
- continuation requests after a crash or a broken response ([recovery.md](recovery.md)).

The caller, and every wrapper between the caller and the provider, sees one call and one outcome:
one `LlmResult`, one stream that ends once, or one `AiError`. Retries, continuations and replay
never re-enter a wrapper as a new exchange. A wrapper's code runs again only when the invocation
replays, and then it sees the same exchange with the same results.

The exchange's `messages` hold what the caller sent and what the model produced. They never hold a
synthetic continuation prompt: a continued answer appears as one assistant message whose text is the
delivered prefix followed by the continuation.

Pinning (Rule 3): at the start of a model call the library reads the model value of every `Shared`
field it uses, the fallback members included, once. The whole call, its retries, continuations and
fallback attempts use those values. A model value obtained with `.get()` or constructed in code stays
pinned while it is held.

## `Llm`

### Input

> The types on this page use the contract's WIT-like notation ([README.md § Notation](README.md#notation)). It is notation only: Golem AI defines no WIT interfaces, and each SDK writes these types by hand in its own idiom. Unless the page's Layer line says otherwise, they are the reference model ([README.md § Layers](README.md#layers)): each SDK exposes their information in its own API design.

```text
variant input {
  text(string),                 // one user message
  messages(list<message>),      // a conversation, for example a previous result's messages
}

record message {
  role: role,
  content: list<part>,
}

enum role { user, assistant, tool }

variant part {
  text(string),
  media(multimodal-part),       // one part of the 4.3 StandardMultimodal value
  tool-call(tool-call),         // only in assistant messages
  tool-result(tool-result),     // only in tool messages
}

record tool-call {
  id: string,                   // the provider's call ID, or one the library assigns
  name: string,                 // the projected tool name (tool-loop.md)
  arguments: json,              // always complete; never a fragment
}

record tool-result {
  call-id: string,
  name: string,
  outcome: result<json, tool-error>,
}

record tool-error {             // the projection's error envelope { "error", "payload" }
  error: string,
  payload: option<json>,
}
```

`multimodal-part` is one element of 4.3's `StandardMultimodal` (`text`, `binary` or `data`, with
optional `media-type`, `filename` and `metadata`; binary data is inline bytes or a URL reference).
The library passes a URL reference through when the provider accepts URLs, and otherwise fetches the
bytes as an ordinary recorded HTTP request of the calling invocation: only `https`, or `http` when a
network grant names the host, up to a size limit \[PROP\] of 20 MiB per part (plan D8). A provider
that accepts neither the media type nor URLs fails the call with `invalid-request` before sending.

### Options

```text
record llm-options {
  system: option<string>,             // [PROP]
  output: option<schema>,             // [PROP] structured output; see below
  tools: list<local-tool>,            // [PROP] interfaces.md#local-tools; empty by default
  step-limit: option<u32>,            // [PROP] default 10
  tool-mode: option<tool-mode>,       // [PROP] default automatic
}

enum tool-mode { automatic, required, none }   // [PROP]
```

The option set is minimal on purpose. Generation parameters (temperature, maximum output tokens,
reasoning effort and similar) are provider options in the model value
([capabilities.md](capabilities.md#model-values)). Per-step policy uses tool middleware,
`RetryPolicy` values, the cancellation signal, and code that calls `runWith` one step at a time.

`output` is the SDK's own schema for the result type (each SDK's schema derivation). With `output`,
the final step asks the provider for structured output in that schema (a JSON-schema response
format where the provider has one, otherwise a single forced tool whose arguments are the value).
The value is decoded and validated against the schema; a value that does not decode fails the call
with `invalid-output`. The library does not repair or retry it.

### Result

```text
record llm-result {
  text: string,                 // the final step's text
  output: option<T>,            // the typed value, when output was given
  messages: list<message>,      // the input messages plus everything the model produced
  steps: list<step>,
  usage: llm-usage,             // the sum over all steps
  finish: finish-reason,        // the final step's reason
  served-by: model-ref,         // the member that produced the result
  attempts: list<attempt>,      // one per fallback member tried, in order
}

record step {
  index: u32,                   // from 0
  text: string,
  tool-calls: list<tool-call>,
  tool-results: list<tool-result>,
  usage: llm-usage,
  finish: finish-reason,
}

enum finish-reason {
  stop,              // the model ended its answer
  length,            // the provider's output limit was reached
  tool-calls,        // the step ended to call tools (never the final step's reason)
  step-limit,        // the step limit was reached; the last step ran without tools
  content-filter,    // the provider stopped for its content policy
}

record model-ref {
  capability: capability,       // llm, transcription, speech, image, embedding
  provider: string,             // the provider key, for example "anthropic"
  model: string,
  member: u32,                  // position in the fallback composite; 0 for a plain model
}

record attempt {
  member: model-ref,
  outcome: attempt-outcome,
}

variant attempt-outcome {
  served,
  failed(ai-error-kind),        // the error that moved the call to the next member
}
```

Reaching the step limit is not an error: the result has `finish: step-limit` and the text of the
last step, which ran without tools ([tool-loop.md](tool-loop.md#steps)).

### Stream events

`stream` and `streamWith` return a stream of `llm-event`. The stream is an ordinary Golem stream
(4.4) and can be returned from a method, consumed in code or projected to text.

```text
variant llm-event {
  step-start(step-start),
  text-delta(string),
  tool-call(tool-call),                     // complete arguments only
  tool-result(tool-result),
  step-end(step-end),
  finish(llm-result),                       // always the last event of a successful stream
}

record step-start { index: u32, served-by: model-ref }
record step-end { index: u32, finish: finish-reason, usage: llm-usage }
```

Rules for every SDK:

- Events arrive in this order per step: `step-start`, then `text-delta` and `tool-call` events in
  the order the model produced them, then `tool-result` events in the order of their calls, then
  `step-end`. After the last step comes exactly one `finish`.
- A `tool-call` event is emitted only when its arguments are complete and parse as JSON. Argument
  fragments and partial structured values are never exposed (Rule 10).
- With `output`, the typed value appears only in `finish`. Its JSON text is not streamed as
  `text-delta`.
- A failed stream ends with the SDK's stream error carrying an `AiError`, after the events that were
  already delivered. A cancelled stream ends in the SDK's cancellation form.
- The concatenated `text-delta` values of a step equal that step's `text` in the final result.

Each SDK provides a projection from `stream<llm-event>` to the text-only stream `stream<string>`
(`text-stream` \[PROP\]) for a method that returns text to HTTP and Durable Streams consumers
(4.4), and to MCP through a tool command. The projection forwards `text-delta` values and ends when
the source ends, with the same error.

### Token counting

- `estimateTokens(text) -> u64` \[PROP\] is local, deterministic and approximate. It uses one
  characters-per-token ratio per provider family and bundles no tokenizer tables (8.3 size rules).
  Every SDK returns the same count for the same text and model value; the shared vectors in the
  fixture corpus check it (plan D7). The algorithm: count the Unicode scalar values of the text,
  divide by the family ratio and round up. The ratios \[PROP\] are 4.0 for `openai`, `grok`,
  `openrouter` and `ollama`, 3.5 for `anthropic` and `bedrock`. An empty text is 0.
- `countTokens(input, options) -> u64` \[PROP\] returns the provider's exact input token count for
  the request that `runWith` would send for its first step, through the provider's count endpoint.
  Only `anthropic` has one in scope (`POST /v1/messages/count_tokens`); every other provider fails
  with `unsupported` (Rule 4). It is a recorded remote call like any provider call. It reserves and
  commits no quota.

## Fallback

`model.fallback(a, b, …)` \[PROP\] returns an `Llm` built from members of one capability type, each
a `Shared` field or an `Llm` value (Rule 2). The same composite exists for `Embedding`
(`model.fallback` over `Embedding` members) \[PROP\]. Members may themselves be composites or
wrappers such as Memory.

- Members are tried in the order given. The first member that succeeds produces the result, and the
  result names it in `served-by`. `attempts` lists every member tried, in order.
- A member's failure moves the call to the next member only when the error is a fallback trigger
  ([errors.md](errors.md#fallback-triggers)) **and** the failure came before the call exposed
  anything: before the first event of a stream reached the caller, and before any tool call of the
  exchange completed. After that point the error is final, so no output is duplicated and no tool
  call is repeated \[PROP\]. This settles 3.2's open "fallback after partial output".
- Each member is called through its own call form (the same form the caller used), with the same
  input, options and toolset. So a wrapper around one member, for example a Memory, is called once per
  member that is tried. Only the member that succeeds returns a result, so only that member's wrapper
  sees a successful exchange.
- When every member fails, the call fails with the last member's error, and its `attempts` lists
  every member.
- Rule 8's run-start check covers every member, because each is a model the run may use
  ([tool-loop.md](tool-loop.md#run-start-check)).

## `Transcription`

```text
record audio {
  data: audio-data,                       // inline bytes or a URL reference (as multimodal-part)
  format: option<audio-format>,           // voice.md#audio-format; required for raw PCM and mu-law
  media-type: option<string>,             // for containers: audio/wav, audio/mpeg, audio/ogg, …
}

record audio-chunk { bytes: list<u8> }    // in the format given in transcribe-options.format

record transcribe-options {
  language: option<string>,               // BCP 47; the provider detects it when absent
  format: option<audio-format>,           // required for transcribeStream
  keywords: list<string>,                 // vocabulary hints, where the provider accepts them
  interim: bool,                          // transcribeStream only: emit interim hypotheses; default false
}

record transcript {
  text: string,
  segments: list<segment>,
  language: option<string>,
  usage: transcription-usage,
  served-by: model-ref,
}

record segment {
  text: string,
  start-ms: u64,
  end-ms: u64,
  speaker: option<string>,
  confidence: option<f32>,
}

variant transcript-event {
  interim(segment),           // a hypothesis that may still change; only when interim = true
  final(segment),             // a stable segment; never changes and is never sent again
  speech-started(u64),        // the provider detected speech at this offset, in ms
  turn-end(u64),              // the provider's end-of-turn or end-of-utterance signal, in ms
  finish(transcript),         // always the last event of a successful stream
}
```

`transcribeStream` takes the caller's audio stream and returns events while the audio arrives.
`final` segments are in order and do not overlap. `turn-end` is the provider's own signal; a
provider without one never emits it ([providers.md](providers.md#r6), [voice.md](voice.md#turns)).
After a crash a transcript stream follows Rule 10's shape (G7; [recovery.md](recovery.md#transcripts)).

## `Speech`

```text
record speak-options {
  voice: option<string>,                  // the provider's voice ID; the model value's default when absent
  format: audio-format,                   // the output format the caller wants
  language: option<string>,
}

record speech-result {
  audio: list<u8>,
  format: audio-format,
  usage: speech-usage,
  served-by: model-ref,
}

variant speech-event {
  audio(list<u8>),                        // in speak-options.format
  mark(u32),                              // the end of the audio for input text item n (speakStream with a text stream)
  finish(speech-result-summary),          // always the last event; usage and served-by, no audio
}

record speech-result-summary { format: audio-format, usage: speech-usage, served-by: model-ref }
```

`speakStream` accepts either one text or a text stream, such as the text projection of a model's
stream. With a text stream, each item is one unit of input, and after the audio for item *n* the
stream emits `mark(n)`. The library converts audio to `format` only when the provider cannot produce
it (for example mu-law from 16-bit linear PCM); conversions are in [voice.md](voice.md#audio-format).
`speak` returns playable audio in a self-describing container when `format` names one (`wav`, `mp3`,
`ogg-opus`) and raw samples otherwise.

## `ImageModel`

```text
record image-request {
  prompt: string,
  references: list<reference-image>,
  count: u32,                             // default 1
  size: option<string>,                   // for example "1024x1024"; the provider's default when absent
  output-format: option<string>,          // image/png (default), image/jpeg, image/webp
}

record reference-image {
  image: multimodal-part,                 // binary image data, inline or a URL reference
  role: reference-role,
}

enum reference-role { subject, style, edit-target, other }

record image-result {
  images: list<multimodal-part>,          // inline bytes, with media-type
  usage: image-usage,
  served-by: model-ref,
}
```

A role the provider cannot honour is not dropped silently: the library sends the reference with the
role written into the prompt where the provider has no role field, as documented per provider in
[providers.md](providers.md#r5). Results are always inline bytes; a provider that returns URLs is
fetched by the library so that the result does not depend on a URL's lifetime.

## `Embedding`

```text
variant embed-input {
  text(string),
  media(multimodal-part),                 // for providers with multimodal embeddings
}

record embed-options {
  input-type: option<input-type>,         // query or document, where the provider distinguishes them
  dimensions: option<u32>,
}

enum input-type { query, document }

record embed-result {
  vectors: list<list<f32>>,               // exactly one vector per input, in input order
  usage: embedding-usage,
  served-by: model-ref,
}

record rerank-request {
  query: string,
  documents: list<string>,
  top: option<u32>,                       // return at most this many
}

record rerank-result {
  ranked: list<ranked>,                   // best first
  usage: embedding-usage,
  served-by: model-ref,
}

record ranked {
  index: u32,                             // the document's index in rerank-request.documents
  score: f32,                             // the provider's relevance score; higher is better
}
```

`embed` returns exactly one vector per input; a provider response with another count fails the call
with `invalid-output`. `rerank` returns each document's index and score, best first, and at most
`top` entries. Ties keep the provider's order. `rerank` uses the model value's reranking model
([capabilities.md](capabilities.md#embedding)); a value without one, or a provider without
reranking, fails with `unsupported`.
