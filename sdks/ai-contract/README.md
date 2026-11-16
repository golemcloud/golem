# Golem AI contract

This directory is the language-neutral contract of Golem AI (spec §3.1). Seven SDKs (Rust,
TypeScript, Effect, Scala, Go, MoonBit and Python) each get their own Golem AI library, written
from scratch in that SDK's idiom. They agree because each one implements the binding layer written
here, and exposes the reference model in its own design ([Layers](#layers); Rules 13 and 15).

The contract owns:

- the five capability types, their model values and the built-in provider list of each;
- the operations and call forms, and their request, result and stream-event types;
- the `AiError` family;
- usage units, quota reservation and commit;
- the tool loop and the function-tool projection that Golem AI uses;
- recovery, cancellation and the execution model, as behaviour a caller can observe;
- the interfaces that wrappers (fallback, Memory) and user-defined providers implement;
- the voice contract that the voice package implements in every SDK;
- the provider capability matrix and the research results behind it.

It owns no code. The library in each SDK owns its types, its transport and its provider adapters.

## Pages

| Page | What it defines |
| -- | -- |
| [capabilities.md](capabilities.md) | Capability types, model values, provider lists, per-SDK spelling |
| [operations.md](operations.md) | Operations, call forms, requests, results, stream events, the logical exchange, fallback, token counting |
| [interfaces.md](interfaces.md) | Local tools, the wrappable `Llm` and `Embedding`, user-defined providers, cancellation, execution model |
| [errors.md](errors.md) | `AiError` kinds, causes and fallback triggers |
| [usage-and-quota.md](usage-and-quota.md) | Usage units, weights, reservation (B2), commit, the no-usage rule, the default meter |
| [tool-loop.md](tool-loop.md) | Bound and exact toolsets, steps, tool errors, the projection, provider tool limits |
| [recovery.md](recovery.md) | Rules 9–11 as observable behaviour, including transcript streams (G7) |
| [voice.md](voice.md) | The voice session contract (plan §8.2.1) |
| [providers.md](providers.md) | The provider capability matrix (R1), the G4 gate, image providers (R5), streaming protocols (R6), the STT accuracy threshold |
| [traceability.md](traceability.md) | Every 3.1 Rule and Done-means item, 3.2's ME-04 list and 3.3's voice list, mapped to the section that defines it |

## Consumers

- **Each SDK's Golem AI library.** The Rust library (`sdks/rust/golem-ai` \[PROP\]) produces the
  canonical capability schemas and the provider fixture corpus from this contract. The TypeScript,
  Effect, Scala, Go, MoonBit and Python libraries are checked against those files, not against the
  Rust code. The Rust library is the reference for those files only, not for API design.
- **Each SDK's voice package**, built on that SDK's Golem AI library.
- **Memory (3.2).** Its AI adapters (ME-04, ME-26–ME-32) wrap an `Llm` and call `Embedding` through
  the interfaces in [interfaces.md](interfaces.md).
- **The phone support agent (3.3)**, through the voice package.
- **The model mock (`golem-model-mock` \[PROP\])**, which serves responses from the fixture corpus.

## How consumers check against it

This directory holds only Markdown today. The checks arrive with the files that later tickets add
next to it:

| File (added by) | Check |
| -- | -- |
| One canonical schema file per capability type (GOL-1353) | Each SDK's tests assert that the schema it emits for a capability type equals the canonical file (Rule 15; plan D2). |
| The provider fixture corpus: requests, responses, streams, errors, rerank responses and `estimateTokens` vectors (GOL-1353, recorded by GOL-1355–GOL-1357) | Each SDK's unit tests replay the same files. A cargo-make task copies them for Scala and MoonBit, and a CI check fails on a stale copy (plan D13). |
| Voice fixtures (GOL-1371) | Each SDK's voice package replays the same audio and event sequences. |
| Projection vectors in `test-data/reflection-conformance/v1.json` (GOL-1359 interim, GOL-1599 final) | Each SDK's existing reflection conformance run checks the Golem AI projection against the same vectors as MCP. |

The conformance scenarios of plan §7 run against an agent and a tool component per SDK under
`test-components/`. Each scenario names the contract section it checks.

A change to this contract updates every in-tree producer and consumer in the same change. There is
no versioning and no compatibility path (repository policy; plan X17).

## Layers

The contract has two layers. Each page starts with a **Layer** line that says which of its parts
belong to which.

**Binding.** Every SDK implements these identically, because they cross SDK boundaries or a shared
file checks them:

- the schema of each capability type's model value, which all seven SDKs emit identically (Rule 15;
  [capabilities.md](capabilities.md));
- the provider list and each provider's wire behaviour: the requests sent and how responses are read
  ([providers.md](providers.md) and the fixture corpus);
- the `AiError` kinds, when each one occurs, and the fallback triggers ([errors.md](errors.md));
- the usage units, the weight, and what is reserved and committed ([usage-and-quota.md](usage-and-quota.md));
- the behaviour rules: toolsets and the function-tool projection ([tool-loop.md](tool-loop.md)),
  recovery ([recovery.md](recovery.md)), pinning, fallback order, stream event order and the other
  observable guarantees on [operations.md](operations.md) and [voice.md](voice.md);
- texts that the fixture corpus fixes byte for byte, such as the continuation instruction.

**Reference model.** The operations, requests, options, results, stream events and interfaces that
the pages write as records. They name the information and the choices that every SDK exposes, and
the fixture corpus writes its expected results in their terms. They do not fix an API. Each SDK
designs its own names, shapes, option styles, builders, and result and event types in its idiom, and
its tests map its types to the reference model. In each SDK's design:

- every piece of information and every choice of the reference model is reachable, though a field may
  become a method, a builder step, a callback or a type parameter;
- conveniences may be added (for example a `generateObject`-style call or a text-only stream helper)
  when they keep the binding behaviour;
- the names that the spec fixes keep their names ([Fixed names and proposed names](#fixed-names-and-proposed-names));
- the wrappable `Llm` and `Embedding` and the provider interfaces ([interfaces.md](interfaces.md)) are
  the contract with Memory (3.2) and with user-defined providers in that SDK, so the SDK fixes their
  shape once and documents it.

The first ticket that builds an SDK's library API starts with a short API sketch: the SDK's spelling
of the reference model and one example agent, reviewed before implementation \[PROP\].

### Example references

Existing AI libraries are examples to learn from, not designs to copy. An SDK's design takes from
them what fits its idiom and Golem, and leaves the rest. The Golem parts (models as config, durable
recovery, quotas, bound tools) have no counterpart in them.

| SDK | Example references |
| -- | -- |
| TypeScript | Vercel AI SDK, TanStack AI, Mastra, OpenAI Agents SDK, Flue |
| Effect | Effect AI (`@effect/ai`) |
| Rust | Rig, genai |
| Python | Pydantic AI, OpenAI Agents SDK |
| Go | Genkit for Go, openai-go |
| Scala | langchain4j and Spring AI (JVM, for design only), sttp-openai |
| MoonBit | none in MoonBit; Rust's and Go's references |

The spec names the TypeScript ones as the leading AI SDKs (§3.1 Why). golem-ai 1.6 is not a design
reference; [providers.md](providers.md) uses it for wire formats only.

## Notation

- **\[PROP\]** marks a name or a choice that the spec marks *(proposed)*, or that this contract
  introduces. It may be renamed without a spec change. A name without a mark is fixed by the spec.
- **\[X\]** marks a departure from the spec, made by a recorded decision.
- Rule *n* is §3.1 Rule *n*. Other sections are cited by number (for example 5.4 Rule 9). Plan
  references (D3a, G6b, X17 and similar) point to the
  [Golem AI development plan](https://linear.app/golem-cloud/document/golem-ai-31-development-plan-revision-31-955a1bb4016b).
- 1.4 F*n* and TC-*n* cite the 1.4 Typed config and Bindings decisions of 10 Nov (the review of
  [David's plan, revision 1](https://linear.app/golem-cloud/document/typed-config-and-bindings-development-plan-revision-1-a70a29fa0e27))
  and that plan's tickets. 2.1 K25 is the 2.1 plan's decision on tool call targets.
- **\[1.4 departure\]** marks a place where 1.4's decisions differ from the current §1.4 or §3.1 text
  and this contract follows the decisions.
- Types are written in a WIT-like notation: `record`, `variant`, `enum`, `option<T>`, `list<T>`,
  `result<T, E>`, `stream<T>`. It is notation only, not a WIT file: Golem AI defines no WIT
  interfaces (plan §12), and each SDK writes these types by hand in its own idiom (plan D2). Some
  forms, such as generics and function types, are not valid WIT. Field names are written in
  kebab-case; each SDK spells them in its own casing. In the manifest, a field uses its canonical
  lowerCamelCase name exactly, for example `rerankModel` (1.4 F8). Each SDK maps `stream<T>` to its
  4.4 stream type (see [interfaces.md](interfaces.md#streams)).

### Fixed names and proposed names

| Fixed by the spec | Marked *(proposed)* in the spec, \[PROP\] here |
| -- | -- |
| `Llm`, `Transcription`, `Speech`, `ImageModel`, `Embedding` | `stream`, `streamWith` |
| `model.llm()`, `model.transcription()`, `model.speech()`, `model.image()`, `model.embedding()` (TypeScript); `Shared<…>` (Rust) | The Effect, Scala, MoonBit, Go and Python spellings of the capability field types |
| `run`, `runWith` (`run_with` in snake-case SDKs) | `transcribe`, `transcribeStream`, `speak`, `speakStream`, `generate`, `embed`, `rerank` |
| The model value fields `provider` and `quota` | Every other model value field, including the reranking-model field and the `config:` catalog schema |
| `AiError` (the family) | Every `AiError` kind name |
| `Multimodal` (4.3) | `model.fallback(…)`, `myProvider.llm(…)`, the memory wrapper returning an `Llm` |
| | The option set (`system`, `output`, `tools`, `stepLimit`, `toolMode`), the last step without tools, the fallback triggers, `agentTool` |
| | The default meter, reserving an estimate, "a model call whose reservation is denied is not sent", the reserve grant spelling `quota(?env) @ <recipient> : reserve : <resource>` |
| | Rejecting a run whose toolset has an argument that projects to a reserved name |
| | The voice package API |

Every other name in this contract (for example `estimateTokens`, `countTokens`, `LlmResult`,
`ToolCall`, `golem-ai.llm`) is introduced here and is \[PROP\].
