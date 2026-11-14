# Traceability

Each row maps one item of the spec (§3.1, captured 2026-10-26 and unchanged on 2026-11-05) or of a
consumer's list to the contract section that defines it. A reviewer checks each row against the spec
text, not against this table.

## §3.1 Rules

| Rule | Spec text (short) | Defined in |
| -- | -- | -- |
| 1 | Provider calls run in the SDK with secret-injected outgoing auth; the credential is neither revealed nor recorded | [providers.md § Auth routes](providers.md#auth-routes-under-54), [capabilities.md § Model values](capabilities.md#model-values) (`credential`), [interfaces.md § User-defined providers](interfaces.md#user-defined-providers), [errors.md § Shape](errors.md#shape) (`message` carries no secret) |
| 2 | A built-in provider's model only through a `Shared` config field of a capability type; catalog vs literal; user-defined providers from their own record type; a constructed model is ordinary | [capabilities.md](capabilities.md) (intro, § Model values, § Composition and user-defined providers), [interfaces.md § User-defined providers](interfaces.md#user-defined-providers). \[1.4 departure\] A model field always names a live-managed catalog entry; literal fills are rejected (1.4 F1, F7) |
| 3 | A catalog value changes as a whole; one model call pins its model and fallbacks | [capabilities.md § Model values](capabilities.md#model-values) (whole values; each read resolves by name and records the revision, 1.4 F10), [operations.md § The model call](operations.md#the-model-call-and-the-logical-exchange) (pinning) |
| 4 | Closed union keyed by `provider` with typed options; deploy check; fallbacks of one capability; the mixed-capability caveat; an unsupported call fails with an explicit `AiError` | [capabilities.md § Model values](capabilities.md#model-values), [operations.md § Operations by capability](operations.md#operations-by-capability), [errors.md § Kinds](errors.md#kinds) (`unsupported`), [providers.md § Matrix](providers.md#matrix) (`transcribeStream` on AWS and `rerank` on OpenAI are `unsupported`) |
| 5 | An endpoint change never widens where a credential may be sent | [capabilities.md § Model values](capabilities.md#model-values) (`credential`: destinations and the deploy check) |
| 6 | Bound form: each bound declaration once, under environment and declaration middleware, plus local tools; exact form: only the tools passed | [tool-loop.md § Toolsets](tool-loop.md#toolsets), [tool-loop.md § The bound toolset](tool-loop.md#the-bound-toolset) (once each through 1.4 TC-09 reflection) |
| 7 | A bound tool call is an ordinary invocation; a failure or denial goes back to the model | [tool-loop.md § The bound toolset](tool-loop.md#the-bound-toolset) (calls, failures), [errors.md § Errors from tools](errors.md#errors-from-tools-inside-a-tool-loop) |
| 8 | Names, schemas and envelopes follow the MCP projection without `idempotency_key`; run-start rejection of duplicates and unacceptable names or schemas across fallback members; reserved names | [tool-loop.md § The projection](tool-loop.md#the-projection), [§ Run-start check](tool-loop.md#run-start-check), [§ Provider limits](tool-loop.md#provider-limits) |
| 9 | A completed model call is never sent again; an in-flight call restarts or continues; idempotent reruns repeat calls; each sent call counts | [recovery.md § Rule 9](recovery.md#model-calls-are-never-sent-again-rule-9), [usage-and-quota.md § Commit](usage-and-quota.md#commit-after-the-call) |
| 10 | After a crash during text, the delivered prefix is kept and a new call supplies the rest; unexposed partial output restarts; completed tool calls do not restart | [recovery.md § Text](recovery.md#text-rule-10), [§ Transcripts](recovery.md#transcripts) |
| 11 | Media not yet exposed restarts internally; exposed media that cannot resume fails explicitly with what was delivered; never corrupt or duplicated | [recovery.md § Media](recovery.md#media-rule-11), [errors.md § Kinds](errors.md#kinds) (`media-interrupted`) |
| 12 | Reported usage counted once against the serving model's `Quota` or the default meter; replay never recounts; interrupted calls; `reserve` authority; a denied reservation is not sent | [usage-and-quota.md](usage-and-quota.md) (`reserve` enforced by 1.4 TC-12), [errors.md § Refused reservations](errors.md#refused-reservations) |
| 13 | Every SDK has the same capabilities, concepts and call forms in its idiom | [README.md](README.md) (consumers, [§ Layers](README.md#layers): the binding layer and the reference model, with example references per SDK), [capabilities.md § Per-SDK spelling](capabilities.md#per-sdk-spelling), [interfaces.md § Streams](interfaces.md#streams), [§ Cancellation](interfaces.md#cancellation) |
| 14 | golem-ai's video, web search, document search, graph and vector databases and code execution stay supported as Rust libraries | Outside this contract by the rule itself. golem-ai keeps those modules (plan §8.3 item 4; GOL-1379 removes only llm, embed, stt and tts) |
| 15 | One provider list per capability for all SDKs; a conformance suite checks the list and schema; widening | [capabilities.md § Built-in providers](capabilities.md#built-in-providers), [README.md § How consumers check](README.md#how-consumers-check-against-it); widening is not implemented \[X\] ([capabilities.md § Model values](capabilities.md#model-values), plan X17) |

## §3.1 Proposed, open to the implementer

| Item | Decided in |
| -- | -- |
| Option set `system`, `output`, `tools`, `stepLimit`, `toolMode`; last step without tools; reaching the maximum is not an error; the default (10) | [operations.md § Options](operations.md#options), [tool-loop.md § Steps](tool-loop.md#steps) |
| Fallback triggers; order; retries under `golem.retry` or a `RetryPolicy` | [errors.md § Fallback triggers](errors.md#fallback-triggers), [operations.md § Fallback](operations.md#fallback) |
| Quotas: default meter per capability, weighted meters, reserve an estimate and commit actual usage | [usage-and-quota.md](usage-and-quota.md) |
| One `AiError` family with shared kinds, carrying memory failures | [errors.md](errors.md) (`foreign`) |
| `agentTool(...)` | [interfaces.md § Local tools](interfaces.md#local-tools) |

## §3.1 Done means

| Item | Defined in |
| -- | -- |
| An agent and a tool call models with no credential, HTTP or quota code; the credential never appears in code, logs, the oplog, REST or CLI output | [capabilities.md § Model values](capabilities.md#model-values), [providers.md § Auth routes](providers.md#auth-routes-under-54), [usage-and-quota.md](usage-and-quota.md) (the library reserves and commits), [errors.md § Shape](errors.md#shape) |
| A catalog switch of provider and model changes the next call, with no code change, rebuild or revision | [capabilities.md § Model values](capabilities.md#model-values) (catalog values), [operations.md § The model call](operations.md#the-model-call-and-the-logical-exchange) (pinning per call). \[1.4 departure\] The switch is a live `golem` CLI or REST edit of the Shared entry, not a `golem deploy` (1.4 F1) |
| Bound and exact forms offer exactly what Rule 6 says; bound tools have MCP's names and schemas; a denied call returns an error to the model and the run continues | [tool-loop.md](tool-loop.md) |
| Structured output returns a typed value; streamed text returned from a method reaches HTTP and Durable Streams consumers | [operations.md § Options](operations.md#options) (`output`), [§ Stream events](operations.md#stream-events) (`text-stream`), [interfaces.md § Streams](interfaces.md#streams) |
| Streaming STT with a provider that streams transcribes a fixed fixture within the accuracy threshold | [providers.md § The STT accuracy threshold](providers.md#the-stt-accuracy-threshold), [§ R6](providers.md#r6) (Deepgram) |
| TTS produces playable audio | [operations.md § Speech](operations.md#speech) |
| Image generation with reference images returns a valid image | [operations.md § ImageModel](operations.md#imagemodel), [providers.md § R5](providers.md#r5) |
| Embeddings return one vector per input | [operations.md § Embedding](operations.md#embedding) |
| Reranking with Cohere and VoyageAI returns each document's index and score, best first | [operations.md § Embedding](operations.md#embedding), [providers.md § Embeddings and reranking](providers.md#embeddings-and-reranking-embedding) |
| Crashes during text and media streams meet Rules 10 and 11 | [recovery.md](recovery.md) |
| Usage appears against each model's `Quota`; replay does not change consumption | [usage-and-quota.md](usage-and-quota.md), [recovery.md § Quota across recovery](recovery.md#quota-across-recovery) |
| A user-defined provider serves calls and reports usage | [interfaces.md § User-defined providers](interfaces.md#user-defined-providers) |
| Every SDK ships the 1.6 providers and at least one image provider; each serves the calls 5.4's auth values can sign (AWS STT without `transcribeStream`) | [capabilities.md § Built-in providers](capabilities.md#built-in-providers), [providers.md § Matrix](providers.md#matrix) |
| Docs, how-to guides and templates in all seven SDKs | Not a contract item: GOL-1378 and each SDK's last library ticket (plan K11) |
| The voice package handles turn-taking and barge-in over a streamed audio fixture; the phone agent (3.3) runs on it | [voice.md](voice.md) |

## 3.2 Memory: the ME-04 list

From the Memory plan (revision 3, 2026-11-09), § Golem AI integration and enrichment.

| Memory needs | Defined in |
| -- | -- |
| The logical exchange: one call across provider attempts, fallback members and recovery; context retrieved and the exchange observed once | [operations.md § The model call and the logical exchange](operations.md#the-model-call-and-the-logical-exchange) |
| Synthetic recovery prompts excluded from what is observed | [operations.md § The model call](operations.md#the-model-call-and-the-logical-exchange) (`messages`), [recovery.md § Text](recovery.md#text-rule-10) |
| Stream events | [operations.md § Stream events](operations.md#stream-events) |
| `AiError` causes, including the foreign cause; Memory failures through the family | [errors.md § Shape](errors.md#shape), [§ Kinds](errors.md#kinds) (`foreign`) |
| `failed-reservation` distinguished from a missing reserve grant | [errors.md § Refused reservations](errors.md#refused-reservations) (`quota-rejected` vs `reserve-denied`) |
| The local tool type, which other libraries construct | [interfaces.md § Local tools](interfaces.md#local-tools) |
| `estimateTokens` and `countTokens` | [operations.md § Token counting](operations.md#token-counting) |
| A cancellation signal on every operation; cancellation never turned into failure or empty success | [interfaces.md § Cancellation](interfaces.md#cancellation) |
| A wrappable `Llm` with every call form (exact and bound, structured results, streams, toolsets, cancellation, recovery) | [interfaces.md § The wrappable Llm and Embedding](interfaces.md#the-wrappable-llm-and-embedding) |
| `runWith` with an empty toolset | [operations.md § Operations by capability](operations.md#operations-by-capability), [interfaces.md § The wrappable Llm and Embedding](interfaces.md#the-wrappable-llm-and-embedding) |
| `Embedding` as its own interface | [interfaces.md § The wrappable Llm and Embedding](interfaces.md#the-wrappable-llm-and-embedding) |
| Fallback attribution to the member that produced the result; only the selected branch observes | [operations.md § Fallback](operations.md#fallback) (`served-by`, `attempts`, one call per member tried) |
| Fallback after partial output (open in the Memory plan) | [operations.md § Fallback](operations.md#fallback): no fallback after anything was exposed or a tool call completed \[PROP\] |
| A bound-form run offers bound memory database tools; the exact form does not | [tool-loop.md § The bound toolset](tool-loop.md#the-bound-toolset) |

## 3.3 Phone agent: the voice list

From plan §5.8 and the 3.3 plan (3c, D4).

| 3.3 needs | Defined in |
| -- | -- |
| `Played` and `Cleared` marks in | [voice.md § Input](voice.md#input) |
| A signal to drop buffered audio out | [voice.md § Output](voice.md#output) (`clear`) |
| A mark for every sentence | [voice.md § Output](voice.md#output) |
| A `TranscriptLine` type kept in agent state | [voice.md § Output](voice.md#output) (`transcript-line`) |
| A session seeded from agent state | [voice.md § Seeding from agent state](voice.md#seeding-from-agent-state) |
| Media recovery: a short silence, a recovery phrase, then the interrupted sentence from its first word | [voice.md § Recovery](voice.md#recovery) |
| STT recovery from kept audio; interim hypotheses restart; final segments kept | [voice.md § Recovery](voice.md#recovery), [recovery.md § Transcripts](recovery.md#transcripts) |
| The audio format and conversions (mu-law 8000 Hz mono; conversion only where a provider needs it) | [voice.md § Audio format](voice.md#audio-format) |
| The providers that stream STT | [voice.md § Which providers stream](voice.md#which-providers-stream), [providers.md § R6](providers.md#r6) |
| Each turn is one `runWith` with local closures | [voice.md § Session](voice.md#session) |
| Turn detection, barge-in, filler | [voice.md § Turns](voice.md#turns) |
| The 1.6 call-center demo's coordination: turn tracking, playback acknowledgements, response attempt IDs, kept audio, provider session replacement | [voice.md § Turns](voice.md#turns) (turns, attempt IDs), [§ Input](voice.md#input) (acknowledgements), [§ Recovery](voice.md#recovery) (kept audio, reconnect after `session-lost`) |
| The turn span | [voice.md § Session](voice.md#session) |
| The latency budget | [voice.md § Latency](voice.md#latency) |
