# Usage and quota

**Layer:** binding, except the usage records, which are reference model.

Golem AI counts the usage that the provider reports for each model call once, against the `Quota` of
the model that served it (Rule 12). A value without a `quota` counts against its capability's
default meter \[PROP\]. This page fixes the units, the weight, what is reserved before a call is sent,
and what is committed after.

The host side is `golem:quota` (`wit/deps/golem-quota/types.wit`): `new-token`, `reserve(token,
amount) -> result<reservation, failed-reservation>`, `reservation.commit(used)`, `split`, `merge`.
`reserve` and `commit` are recorded host calls (`WriteRemote` in
`golem-worker-executor/src/durable_host/quota/mod.rs`), so replay returns the recorded result and
never counts a completed call again. Dropping a reservation without committing commits zero.

## Usage records

Each result carries the usage the provider reported, in the provider's units:

> The types on this page use the contract's WIT-like notation ([README.md § Notation](README.md#notation)). It is notation only: Golem AI defines no WIT interfaces, and each SDK writes these types by hand in its own idiom. Unless the page's Layer line says otherwise, they are the reference model ([README.md § Layers](README.md#layers)): each SDK exposes their information in its own API design.

```text
record llm-usage {
  input-tokens: option<u64>,     // every input token the provider reports, cache reads and writes included
  output-tokens: option<u64>,    // reasoning tokens included
}

record transcription-usage { audio-ms: option<u64> }
record speech-usage { characters: option<u64> }
record image-usage { images: option<u32> }
record embedding-usage {
  input-tokens: option<u64>,     // embed
  rerank-units: option<f64>,     // rerank, in the provider's unit (see below)
}
```

A field is absent when the provider did not report it and the library could not count it exactly.

## Units and the weight (plan D3a)

| Capability | Unit counted | Source |
| -- | -- | -- |
| `Llm` | input tokens + output tokens | The provider's usage fields, summed over every step and continuation of the call |
| `Transcription` | audio seconds | The provider's reported duration; otherwise the duration of the audio sent, computed from its format |
| `Speech` | input characters | The provider's reported count (Polly `x-amzn-RequestCharacters`, Deepgram `dg-char-count`, ElevenLabs `character-cost`); otherwise the Unicode scalar values of the text sent |
| `ImageModel` | images | The number of images returned |
| `Embedding` (`embed`) | input tokens | The provider's usage field |
| `Embedding` (`rerank`) | the provider's unit | Cohere: `meta.billed_units.search_units` when present, otherwise `meta.billed_units.input_tokens`; VoyageAI: `usage.total_tokens`; Hugging Face: none reported, so the no-usage rule applies (the documents count) |

Every model value has an optional weight \[PROP\] (default 1, a positive number). The quantity a call
commits is

```text
committed = ⌈ (sum of the call's units) × weight ⌉
```

computed in IEEE 754 binary64 and rounded up once, never per part: for an `Llm` call
⌈(input + output) × weight⌉. Audio seconds enter the sum as `audio-ms / 1000`. A deployer who puts
several providers on one quota resource sets the weights to bring them to a unit of their choice,
such as cents. One model value has one model, so the unit of a value is fixed; the weight is set per
value.

## Reservation before sending (plan D3b, B2)

Before a model call sends anything, the library reserves on the model's quota token, on the calling
invocation's task:

| Call | Reserved amount |
| -- | -- |
| `run`, `runWith`, `stream`, `streamWith` | the token's expected use |
| `transcribe` | ⌈audio seconds × weight⌉ when the duration is known from the input (raw PCM or mu-law with a format, or a WAV header); otherwise the token's expected use |
| `transcribeStream` | the token's expected use, then re-reserved (below) |
| `speak` | ⌈characters of the text × weight⌉ |
| `speakStream` | for one text, ⌈characters × weight⌉; for a text stream, the token's expected use, then re-reserved |
| `generate` | ⌈count × weight⌉ |
| `embed` | the token's expected use |
| `rerank` | Hugging Face: ⌈documents × weight⌉; Cohere: ⌈⌈documents / 100⌉ × weight⌉; VoyageAI: the token's expected use |
| `countTokens`, `estimateTokens` | nothing; they are not counted (plan D7) |

The token's expected use is the `expected-use` the token was created with: for a model value with a
`quota`, the `Quota`'s own token; for the default meter, the library constant below. A `quota` fill
is a catalog resource name or `{ resource, expectedUse }` (1.4 F4); with a bare name, 1.4 decides the
expected use.

A tool loop reserves once per provider request (each step and each continuation), and commits each
request's usage before the next step reserves. A fallback member reserves on its own token when it is
tried.

**Long sessions.** `transcribeStream` and `speakStream` with a text stream keep one open reservation.
The library measures usage as it sends (audio seconds sent, characters sent). When the measured usage
since the reservation reaches the reserved amount, it commits that reservation with the measured
amount and reserves again before it sends more. A refused re-reservation follows the refusal table in
[errors.md](errors.md#refused-reservations): `reserve-denied` and `quota-rejected` end the session
with that error after the output already delivered; `throttle` waits.

**A refused reservation is never sent** \[PROP\]. See [errors.md](errors.md#refused-reservations)
for which refusals fall back.

## Commit after the call

| How the call ended | Committed |
| -- | -- |
| Success with reported usage | ⌈reported units × weight⌉ |
| Success, but the provider reports no usage (the no-usage rule) | the reserved amount: the exact count where it was known (documents for Hugging Face rerank, characters, images, batch audio length), otherwise the token's expected use |
| The provider answered with an error response | 0 |
| Nothing was sent (a refused reservation, `unsupported`, `invalid-request` found before sending, a connection that never opened) | 0 |
| The request was sent and the call ended before the provider reported usage (a broken response, `media-interrupted`, cancellation after sending) | the reserved amount \[PROP\] (Rule 12: "unless an estimate was reserved for it") |
| A crash, followed by recovery | the interrupted request commits its reserved amount (it was sent and reported nothing); the continuation or restarted request is a new provider request and reserves and commits on its own, as above |

Usage beyond the reserved amount is settled by `commit` (overuse). Today `commit` floors the
remaining allocation at 0. 1.4 TC-12 records overuse as debt (F24; whether debt is per holder or per
resource is David's open question). Either way the library commits the same amounts, so this does
not change this contract.

## The default meter (plan G6a)

A model value without a `quota` reserves on its capability's default meter \[PROP\]: one ordinary quota
resource per capability, with a fixed name. The library gets its token with `acquireQuotaToken(name,
expected-use)`.

| Capability | Resource name \[PROP\] | Expected use \[PROP\] |
| -- | -- | -- |
| `Llm` | `golem-ai.llm` | 4096 |
| `Transcription` | `golem-ai.transcription` | 60 (seconds) |
| `Speech` | `golem-ai.speech` | 1000 (characters) |
| `ImageModel` | `golem-ai.image` | 1 |
| `Embedding` | `golem-ai.embedding` | 8192 |

Until a deployer creates the resource, the token draws on 1.4's unbounded pool. A deployer creates
it like any quota resource: as a `$kind: resource` entry of the `config.<env>` catalog (1.4 F4;
`resourceDefaults` is gone) or with `golem resource create`. Consumption reads back through `golem
resource` like any resource.

## Authority

Every reservation needs `quota(?env) @ <recipient> : reserve : <resource>` \[PROP spelling\] in the
effective authority of the invocation that the reservation host call is attributed to (Rule 12; plan
G5): its principal's wallet, its invocation wallet and its creation cards, within the principal's
limits. Inside an agent-hosted tool, the tool's own authority decides, never the hosting agent's. The
default card grants `reserve` on every quota resource of the environment; a declared card can narrow
it. A caller can lend `reserve` for one call by attaching a card to a local tool's client call.
Enforcement is 1.4 TC-12 (formerly M8): the host checks `reserve` before the 0-unit shortcut in
`try_grant`, on the authority that 5.3 GOL-1290 defines, and the 3.1 test rows ignored until then are
re-enabled in the same change (plan K3). Whatever form the host gives a denial, the library maps it to
`reserve-denied`.
