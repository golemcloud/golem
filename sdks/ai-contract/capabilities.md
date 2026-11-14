# Capability types and model values

**Layer:** binding: the model value types, the provider keys, the credential types and the catalog
rules. Reference model: the spellings marked \[PROP spelling\] and the Effect, Scala, MoonBit, Go
and Python field types.

Code reads models through config fields of a capability type (1.4): `Llm`, `Transcription`, `Speech`,
`ImageModel` or `Embedding`. Capability constructors declare `Shared` fields (`model.llm()` declares
`Shared<Llm>`), and code reads the current value with `this.config.<field>.get()` (1.4 Rule 5). Code
gets a built-in provider's model only through such a field (Rule 2).

## Per-SDK spelling

| Capability | TypeScript | Effect \[PROP\] | Rust | Scala, MoonBit, Go, Python \[PROP\] |
| -- | -- | -- | -- | -- |
| language model | `model.llm()` | `Model.llm()` | `Shared<Llm>` | `Shared[Llm]` |
| speech-to-text | `model.transcription()` | `Model.transcription()` | `Shared<Transcription>` | `Shared[Transcription]` |
| text-to-speech | `model.speech()` | `Model.speech()` | `Shared<Speech>` | `Shared[Speech]` |
| image generation | `model.image()` | `Model.image()` | `Shared<ImageModel>` | `Shared[ImageModel]` |
| embeddings | `model.embedding()` | `Model.embedding()` | `Shared<Embedding>` | `Shared[Embedding]` |

Scala follows 8.1 for its spelling. Call forms follow each language's casing: `run` / `runWith`, or
`run` / `run_with`.

## Model values

Per environment, a model value is a descriptor. Models do not run on Golem.

> The types on this page use the contract's WIT-like notation ([README.md § Notation](README.md#notation)). It is notation only: Golem AI defines no WIT interfaces, and each SDK writes these types by hand in its own idiom. Unless the page's Layer line says otherwise, they are the reference model ([README.md § Layers](README.md#layers)): each SDK exposes their information in its own API design.

```text
record <provider>-<capability>-value {     // one branch of the capability's union
  provider: "<provider key>",              // the discriminator
  model: string,                           // [PROP]
  credential: <credential type>,           // [PROP] a Golem Secret; see the provider table
  endpoint: option<string>,                // [PROP] the base URL; the provider's default when absent
  quota: option<Quota>,
  weight: option<f64>,                     // [PROP] default 1; usage-and-quota.md
  options: option<<provider options>>,     // [PROP] typed per provider
}
```

- **The union.** Each capability type is a closed union with one branch per built-in provider,
  selected by `provider` (a `FieldEquals("provider")` discriminator), with typed options (Rule 4).
  `golem deploy` checks a value like any config value (1.4 Rule 3). All seven SDKs emit the same
  schema for each capability type (Rule 15); GOL-1353 writes the canonical schema files.
- **No widening** \[X\] (plan X17). Rule 15 says that adding a provider widens the union and that the
  widened type keeps the same catalog value. This contract does not implement that: a capability
  type is an ordinary config type, so an SDK that adds a provider or an option emits a different type.
  1.4 stores one schema per catalog name (F11), so the different type needs a new catalog name. Older
  agents keep the old name and its value; moving them to a new provider means updating them to the
  new name.
- **Whole values.** A catalog value changes as a whole, never field by field (Rule 3).
- **`credential`** is a `Secret` handle (5.4), so rotation, cards and per-request revision pinning
  apply. Inside a catalog value it is written `{ $secret: <path> }`; a bare path is rejected (1.4 F3).
  The `hold` the library needs is granted on that secret path. The library never reveals it; each
  adapter places it with 5.4's stand-ins or auth values (plan D9;
  [providers.md](providers.md#matrix)). An endpoint change never widens where the credential may be
  sent (Rule 5): the secret's destinations decide, and `golem deploy` fails when the provider's origin
  is not among them (5.4 OA3).
- **`quota`** is a `Quota` (1.4). A value without one counts against its capability's default meter
  ([usage-and-quota.md](usage-and-quota.md#the-default-meter-plan-g6a)).
- **`options`** hold the provider's generation and request parameters (for example temperature,
  maximum output tokens, voice, sample rate, region, bucket). Each provider's option record is part of
  the canonical schema; GOL-1353 fixes the fields from the matrix in [providers.md](providers.md).
- **Mixed-up capabilities.** Deploy cannot tell apart a value whose fields fit two capability types,
  such as one provider's speech-to-text and text-to-speech. Filled into the wrong capability's field,
  it passes deploy and fails only when the provider rejects the call (Rule 4).

A model field always names a catalog entry; a literal fill is rejected (1.4 F7). A package whose
code uses `model.llm()` ships a named catalog entry for it. The entry is a live-managed Shared value
(1.4 F1): its YAML gives only a default, used when the environment has no value yet. After that the
value changes through `golem` CLI or REST edits and reaches an agent at its next read, without
`golem deploy`, a component revision or an agent update. A rollback keeps the current value (F9).
Each read resolves the field by name and records the ID and revision it saw (F10); reads come from
the environment-state cache, which a registry event clears (F12). Models have no command of their
own.

\[1.4 departure\] §3.1 Rule 2 still distinguishes catalog fills from literal fills, and 1.4's spec
text (Rules 5 and 7) still changes catalog values only through `golem deploy` or a rollback. This
contract follows the 1.4 decisions of 10 Nov (F1, F7, F9), which John approved; the spec text has
not been updated yet.

## Built-in providers

Each capability type's built-in providers are one list shared by all SDKs (Rule 15). The keys are
\[PROP\].

| Capability | Provider keys | Credential type |
| -- | -- | -- |
| `Llm` | `anthropic`, `bedrock`, `grok`, `ollama`, `openai`, `openrouter` | `Secret<string>` (an API key); `bedrock`: `aws-credential`; `ollama`: optional |
| `Embedding` | `cohere`, `hugging-face`, `openai`, `voyageai` | `Secret<string>` |
| `Transcription` | `aws`, `azure`, `deepgram`, `google`, `openai` (1.6's Whisper) | `Secret<string>`; `aws`: `Secret<aws-keys>` |
| `Speech` | `aws`, `deepgram`, `elevenlabs`, `google` | `Secret<string>`; `aws`: `Secret<aws-keys>` |
| `ImageModel` | `google`, `openai` | `Secret<string>` |

```text
record aws-keys {                        // 5.4 awsSigV4 secret shape
  access-key-id: string,
  secret-access-key: string,
  session-token: option<string>,
}

variant aws-credential {                 // [PROP] Bedrock accepts either
  keys(Secret<aws-keys>),                // signed with awsSigV4 (5.4 M2)
  api-key(Secret<string>),               // a long-term Bedrock API key, sent with bearer() (5.4 M1)
}
```

These are the providers that 1.6 golem-ai lists, cross-checked against golem-ai `main` (`llm/`,
`embed/`, `stt/`, `tts/`), plus two image-generation providers (R5). Reranking is part of
`Embedding`: `cohere`, `voyageai` and `hugging-face` rerank; `openai` does not (Rule 4).

## `Embedding`

An `Embedding` value names its reranking model in an optional field, `rerank-model` \[PROP\]. Only the
branches whose provider reranks have it (`cohere`, `voyageai`, `hugging-face`). `rerank` on a value
without it fails with `unsupported`, as does `rerank` on `openai`. There is no default reranking
model, so a stale default cannot pick the wrong one. For `hugging-face`, `endpoint` selects a
self-hosted Text Embeddings Inference server or an Inference Endpoint; without it, the hosted HF
Inference service is used ([providers.md](providers.md#g4)).

## Composition and user-defined providers

- `model.fallback(this.config.main, this.config.backup)` \[PROP spelling\] returns an `Llm`; the same
  for `Embedding` ([operations.md](operations.md#fallback)). Fallbacks combine models of one
  capability type.
- A memory wrapper (3.2) returns an `Llm` \[PROP\] ([interfaces.md](interfaces.md#the-wrappable-llm-and-embedding)).
- Composition accepts both `Shared` model fields and values: a field is read at each call, a value
  stays pinned while held (1.4 Rule 5).
- A model field may be optional, for example `fallbackModel: model.llm().optional()` (1.4 F6); an
  unfilled optional field reads as absent. A list of model fields is rejected (lists of managed roots,
  F6), so each fallback member is its own field.
- A user-defined provider's model is constructed in code from a config field of the provider's own
  record type, whose credential is a `Secret` and whose quota is a `Quota` (Rule 2):
  `myProvider.llm(this.config.mine.get())` \[PROP spelling\]. It is an ordinary model of its
  capability ([interfaces.md](interfaces.md#user-defined-providers)).

## Example

Secrets, quota resources and model values share the one `config.<env>` catalog (1.4 F2, F4). The
kind markers and the default key below follow the 1.4 suggestion (`$kind`, `$default`); David
chooses the final spelling (F4a), and TC-01 and TC-28 own the grammar (F2a).

```yaml
config:
  prod:
    credentials:
      anthropic: { $kind: secret, $destinations: [https://api.anthropic.com] }
      openai:    { $kind: secret, $destinations: [https://api.openai.com] }
      deepgram:  { $kind: secret, $destinations: [https://api.deepgram.com, wss://api.deepgram.com] }
    llm-tokens:
      $kind: resource
      $default: { limit: { type: Capacity, value: 1000000 }, enforcementAction: reject, unit: token, units: tokens }
    claude:
      $kind: shared
      $default: { provider: anthropic, model: <model>, credential: { $secret: credentials.anthropic }, quota: llm-tokens }
    gpt:
      $kind: shared
      $default: { provider: openai, model: <model>, credential: { $secret: credentials.openai }, quota: { resource: llm-tokens, expectedUse: 2048 }, weight: 0.4 }
    ears:
      $kind: shared
      $default: { provider: deepgram, model: nova-3, credential: { $secret: credentials.deepgram }, options: { endpointing: 300 } }
agents:
  Support:
    config: { main: { $config: claude }, backup: { $config: gpt }, ears: { $config: ears } }
```

A `quota` is a resource name or `{ resource, expectedUse }` ([usage-and-quota.md](usage-and-quota.md)).
`ears` has no `quota`, so it counts against `golem-ai.transcription`. The secrets have no
`$default`, so `golem secret create|update` sets their values. Destination semantics, including
whether a `wss://` destination is listed separately from `https://` for the same host, are
GOL-1138's (F2b).
