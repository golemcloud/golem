# Providers

**Layer:** binding: the provider matrix, endpoints, auth routes, streaming protocols and the STT
accuracy threshold. The research notes are background.

This page records the provider research (plan R1, R5, R6 and the G4 gate) and the calls each built-in
provider makes. It was researched on 2026-11-10 from the providers' API references and from
golemcloud/golem-ai `main` at `0923caf` (2026-10-22), which is a wire-format reference only. Each
provider row is confirmed against real responses when its recordings are made (GOL-1353,
GOL-1355–GOL-1357); a recording that contradicts a row changes this page in the same change.

## Auth routes under 5.4

A provider call can authenticate only in the ways 5.4 offers. The host substitutes stand-ins in header
values, URL paths, queries and user info, and on WebSocket only at connect; never in a request body.

| Route | 5.4 milestone | Used by |
| -- | -- | -- |
| `bearer(secret)` in `Authorization` | M1 (GOL-1144) | `openai`, `grok`, `openrouter`, `cohere`, `hugging-face`, `voyageai`, `bedrock` with an API key |
| `"Token " + disguise(secret)` in `Authorization` | M1 | `deepgram` (HTTP and WebSocket connect) |
| `disguise(secret)` in a named header | M1 | `anthropic` (`x-api-key`), `azure` (`Ocp-Apim-Subscription-Key`), `elevenlabs` (`xi-api-key`, HTTP and WebSocket connect), `google` (`x-goog-api-key`) |
| `awsSigV4` | M2 (GOL-1149) | `bedrock` with keys, `aws` (Transcribe, S3, Polly) |
| none | — | `ollama` (an optional `bearer()` for a proxy in front of it) |

WebSocket connect substitution is GOL-1143, which M1 includes. No built-in provider uses
`basicSecret` or `oauthClientCredentials`. Header stand-ins are enough for every key header on the
list, because `disguise` works in any header value: Azure Speech's `Ocp-Apim-Subscription-Key`
included. Azure OpenAI's `api-key` header would work the same way, but Azure OpenAI is not on the
list; the `openai` branch with an `endpoint` reaches Azure's v1 API with `bearer()`.

**Flagged providers.** No provider on the list needs a secret in the request body, the RFC 7523 JWT
grant or STS in the route chosen here. Three routes that do were left out:

- **Google service accounts.** golem-ai 1.6's Google STT and TTS sign an RS256 JWT and POST it as
  `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer&assertion=…` to
  `https://oauth2.googleapis.com/token` (`stt/google/src/transcription/gcp_auth.rs`,
  `tts/google/src/client.rs`): the RFC 7523 grant, with the signed assertion in a body. 5.4 has
  neither. 2.0 uses Google's API-key route instead. Vertex AI, which also needs the grant, is not on
  the list; its express mode accepts API keys but serves only Gemini text and image models.
- **Azure `issueToken`.** Exchanging a subscription key for a bearer token returns the token in a
  response body for later use in a header. The library sends `Ocp-Apim-Subscription-Key` directly.
  golem-ai 1.6 does the same.
- **AWS short-term credentials.** STS and Bedrock short-term API keys (presigned URLs) are not
  available. Long-term IAM keys (optionally with a session token the deployer supplies) or a
  long-term Bedrock API key are used.

ElevenLabs accepts the key either in the `xi-api-key` header or as `xi_api_key` in the first
WebSocket message. Only the header is used, because 5.4 never substitutes a body.

## Matrix

"✓" means the call is supported. Every other call fails with the `AiError` named in the cell, before
anything is sent. Every provider supports `estimateTokens` (`Llm`) locally; it is not repeated below.

### Language models (`Llm`)

| Provider | `run`/`runWith`/`stream`/`streamWith` | `countTokens` | Endpoint | Auth (5.4) | Streaming protocol | Usage fields |
| -- | -- | -- | -- | -- | -- | -- |
| `anthropic` | ✓ | ✓ `POST /v1/messages/count_tokens` | `POST https://api.anthropic.com/v1/messages`, `anthropic-version: 2023-06-01` | `x-api-key` disguise (M1) | SSE: `message_start`, `content_block_start`/`_delta`/`_stop` (`text_delta`, `input_json_delta` buffered per block), `message_delta`, `message_stop` | `input_tokens` + `cache_creation_input_tokens` + `cache_read_input_tokens`; `output_tokens` |
| `bedrock` | ✓ | `unsupported` | `POST https://bedrock-runtime.{region}.amazonaws.com/model/{model}/converse` and `/converse-stream` | `awsSigV4` service `bedrock` (M2), or a Bedrock API key with `bearer()` (M1) | AWS event stream (`application/vnd.amazon.eventstream`: binary frames with prelude, headers and CRC32), parsed incrementally | `usage.inputTokens`, `outputTokens` (stream: `metadata` event) |
| `grok` | ✓ | `unsupported` | `POST https://api.x.ai/v1/chat/completions` | bearer (M1) | SSE chat-completion chunks, `stream_options.include_usage: true`, `data: [DONE]` | `prompt_tokens`, `completion_tokens` |
| `ollama` | ✓ | `unsupported` | `POST {endpoint}/api/chat` (`endpoint` required) | none | NDJSON lines; the `done: true` line carries metadata | `prompt_eval_count`, `eval_count` |
| `openai` | ✓ | `unsupported` | `POST {endpoint}/chat/completions`, default `https://api.openai.com/v1` \[PROP\] | bearer (M1) | SSE chat-completion chunks, `stream_options.include_usage: true`, `data: [DONE]` | `prompt_tokens`, `completion_tokens` |
| `openrouter` | ✓ | `unsupported` | `POST https://openrouter.ai/api/v1/chat/completions` | bearer (M1) | SSE chat-completion chunks with usage in the final chunk | `prompt_tokens`, `completion_tokens` |

- `openai` speaks Chat Completions, not the Responses API that golem-ai 1.6 uses, so that the same
  provider serves OpenAI-compatible endpoints and the shared model mock (GOL-1352, a CI model value
  `provider: openai` with `endpoint` at the mock) \[PROP\].
- `bedrock` limits: the 32 MiB `awsSigV4` body bound (5.4; a larger body fails with `egress-auth`
  `body-too-large`, so the library rejects it first with `invalid-request`), and Bedrock's own: at
  most 20 images of 3.75 MB each and 5 documents of 4.5 MB each per request, images only PNG, JPEG,
  GIF or WebP, and a 25 MB `InvokeModel` body. golem-ai 1.6 buffers the whole response before parsing
  the event stream; 2.0 parses frames as they arrive.
- Tool name and schema limits are in [tool-loop.md](tool-loop.md#provider-limits).

### Embeddings and reranking (`Embedding`)

| Provider | `embed` | `rerank` | Endpoints | Auth (5.4) | Streaming | Usage fields |
| -- | -- | -- | -- | -- | -- | -- |
| `cohere` | ✓ | ✓ | `POST https://api.cohere.com/v2/embed`; `POST https://api.cohere.com/v2/rerank` (`top_n`) | bearer (M1) | none | embed: `meta.billed_units.input_tokens`; rerank: `meta.billed_units.search_units`, else `input_tokens` |
| `hugging-face` | ✓ | ✓ (hosted confirmed by the [G4 call](#g4); `rerank-model` must be mapped to `text-ranking`) | embed: `POST https://router.huggingface.co/hf-inference/models/{model}/pipeline/feature-extraction`; rerank hosted: `POST https://router.huggingface.co/hf-inference/models/{rerank-model}` with `{"inputs":{"query","texts"}}`; rerank and embed self-hosted: `POST {endpoint}/rerank` with `{"query","texts"}`, `POST {endpoint}/embed` (TEI) | bearer (M1) | none | none: the no-usage rule applies |
| `openai` | ✓ | `unsupported` (Rule 4) | `POST {endpoint}/embeddings`, `input` as an array, `dimensions` | bearer (M1) | none | `usage.prompt_tokens` |
| `voyageai` | ✓ | ✓ | `POST https://api.voyageai.com/v1/embeddings`; `POST https://api.voyageai.com/v1/rerank` (`top_k`) | bearer (M1) | none | `usage.total_tokens` |

- `input-type` maps to Cohere `search_query`/`search_document` and VoyageAI `query`/`document`. Cohere
  v3+ models require it; the library sends `search_document` when the caller gives none \[PROP\].
- Rerank responses: Cohere `results[{index, relevance_score}]`; VoyageAI `data[{index,
  relevance_score}]`; Hugging Face and TEI `[{index, score}]`. The library sorts by score, best first,
  and applies `top` itself when the provider has no top parameter.
- Do not copy from golem-ai 1.6: its OpenAI embed concatenates every input into one `input` string and
  sends `dimension`; its Cohere rerank default model is a VoyageAI name (`rerank-2-lite`) and it never
  reads `search_units`. The contract requires one vector per input and has no default reranking model.

### Speech-to-text (`Transcription`)

| Provider | `transcribe` | `transcribeStream` | Endpoints | Auth (5.4) | Streaming protocol | Usage fields |
| -- | -- | -- | -- | -- | -- | -- |
| `aws` | ✓ (needs `options.bucket` and `options.region`) | `unsupported` (Rule 4's own example) | S3 `PUT https://{bucket}.s3.{region}.amazonaws.com/{key}`; `POST https://transcribe.{region}.amazonaws.com/` with `X-Amz-Target: com.amazonaws.transcribe.Transcribe.StartTranscriptionJob`, then `GetTranscriptionJob` polled, `DeleteTranscriptionJob`; `GET` of the job's presigned `TranscriptFileUri` | `awsSigV4` services `s3` and `transcribe` (M2) | none. Transcribe streaming needs per-chunk signatures (HTTP/2) or a presigned `wss://` URL, and 5.4 has neither | the last item's `end_time` |
| `azure` | ✓ | `unsupported`: the real-time WebSocket protocol is undocumented (Speech SDK only) | `POST https://{region}.api.cognitive.microsoft.com/speechtotext/transcriptions:transcribe?api-version=2025-10-15` (Fast Transcription, multipart `audio` + `definition`) | `Ocp-Apim-Subscription-Key` disguise (M1) | none | `durationMilliseconds` |
| `deepgram` | ✓ | ✓ | `POST https://api.deepgram.com/v1/listen`; `wss://api.deepgram.com/v1/listen` (Nova); `wss://api.deepgram.com/v2/listen` (Flux, `model=flux-general-en`) | `Authorization: Token` disguise, HTTP and WebSocket connect (M1) | WebSocket: binary audio frames in; `Results` (`is_final`, `speech_final`), `SpeechStarted`, `UtteranceEnd`, `Metadata` out; `KeepAlive`, `Finalize`, `CloseStream` control. Flux: `TurnInfo` (`StartOfTurn`, `EagerEndOfTurn`, `TurnResumed`, `EndOfTurn`) | `metadata.duration` |
| `google` | ✓ up to Google's inline audio limit; longer audio fails with `invalid-request` | `unsupported`: `StreamingRecognize` is gRPC only | `POST https://speech.googleapis.com/v1/speech:recognize` (up to 60 s); `POST …/v1/speech:longrunningrecognize` with inline content and polling of `operations/{name}` (longer audio within the inline size limit; confirmed at recording) | `x-goog-api-key` disguise (M1) | none | `totalBilledTime` |
| `openai` (1.6's Whisper) | ✓ | `unsupported`: `whisper-1` cannot stream | `POST {endpoint}/audio/transcriptions` (multipart, `model` from the value, `response_format=verbose_json`) | bearer (M1) | none | `usage.seconds` or `duration` |

- `aws` batch: the audio is uploaded as one signed `PutObject` (so at most 32 MiB), the job is polled
  with durable sleeps, and the job and object are deleted afterwards. The secret's destinations list
  the bucket host and the Transcribe host; fetching `TranscriptFileUri` needs no secret but needs a
  network grant for the S3 host.
- `google` with API keys is confirmed for `speech:recognize` v1 (the discovery document's `key`
  parameter and Google's own labs), not for v2; 2.0 uses v1. golem-ai 1.6 uses v2 with service-account
  auth and uploads long audio to Cloud Storage, which needs OAuth; neither is available, so 2.0 accepts
  only inline audio.
- `azure` Fast Transcription accepts audio up to 2 hours and 250 MB; the library is bound by agent
  memory, not by a 5.4 limit, because the route is not `awsSigV4`.
- `openai` (Whisper) accepts files up to 25 MB.
- OpenAI Realtime transcription (`wss://api.openai.com/v1/realtime?intent=transcription`, bearer at
  connect) and ElevenLabs Scribe realtime would both fit 5.4 and could later add `transcribeStream`
  for `openai` or a new `elevenlabs` branch. They are not in the 2.0 minimum and not in this contract.

### Text-to-speech (`Speech`)

| Provider | `speak` | `speakStream` | Endpoints | Auth (5.4) | Streaming protocol | Usage fields |
| -- | -- | -- | -- | -- | -- | -- |
| `aws` (Polly) | ✓ | ✓ one request per input item | `POST https://polly.{region}.amazonaws.com/v1/speech` (`SynthesizeSpeech`) | `awsSigV4` service `polly` (M2) | none: the response body is read as it arrives. `StartSpeechSynthesisStream` (HTTP/2 bidirectional event stream) is not used: it needs an event-stream client with per-event signing, which 5.4 does not provide | `x-amzn-RequestCharacters` |
| `deepgram` | ✓ | ✓ | `POST https://api.deepgram.com/v1/speak?model=…&encoding=…&sample_rate=…`; `wss://api.deepgram.com/v1/speak` | `Authorization: Token` disguise (M1) | WebSocket: `Speak`, `Flush` (server replies `Flushed`; at most 20 per 60 s), `Clear`, `Close`; binary audio out | `dg-char-count` (HTTP); characters sent (WebSocket) |
| `elevenlabs` | ✓ | ✓ | `POST https://api.elevenlabs.io/v1/text-to-speech/{voice}?output_format=…`; `wss://api.elevenlabs.io/v1/text-to-speech/{voice}/stream-input?model_id=…&output_format=…` | `xi-api-key` disguise, HTTP and WebSocket connect (M1) | WebSocket: JSON `text` messages with `flush`; base64 audio out | `character-cost` header (HTTP); characters sent (WebSocket) |
| `google` | ✓ | ✓ one request per input item | `POST https://texttospeech.googleapis.com/v1/text:synthesize` | `x-goog-api-key` disguise (M1) | none: `StreamingSynthesize` is gRPC only | characters sent |

- **One request per input item** \[PROP\]: for a provider without a streaming protocol, `speakStream`
  sends one `speak` request for each input text item and emits its audio and `mark` as the response
  arrives. The events are the same as with a streaming provider; only the latency differs. This is not
  Rule 4's case, because the provider can make every call the operation needs.
- Output formats: Deepgram and ElevenLabs (`ulaw_8000`, `pcm_16000`) and Google (`MULAW`, `LINEAR16`)
  produce mu-law 8000 Hz directly; Polly produces PCM at 8000 or 16000 Hz, which the library converts.
  golem-ai 1.6 maps ElevenLabs mu-law to `pcm_22050`; 2.0 asks for `ulaw_8000`.
- Text limits per request: Polly 3000 billed characters, Google 5000 bytes, Deepgram Aura-2 2000
  characters, ElevenLabs per model (2500–5000). The library splits longer text at sentence boundaries
  and sends the parts in order.
- Polly with a session token sends `X-Amz-Security-Token`; golem-ai 1.6 wrongly folds it into the
  credential scope.
- `google` with API keys is not yet confirmed for Text-to-Speech; Google documents only OAuth for it.
  The recording for GOL-1355 confirms it. If Google refuses API keys for TTS, `google` speech needs the
  RFC 7523 grant, which is a 5.4 gap: the `google` `Speech` branch then fails every call with
  `authentication` until 5.4 adds the grant, and the gap is raised with 5.4.

### Image generation (`ImageModel`)

| Provider | `generate` | Reference images | URLs accepted | Endpoint | Auth (5.4) | Usage |
| -- | -- | -- | -- | -- | -- | -- |
| `google` (Gemini API) | ✓ | up to 14 per request; roles: object (`subject`) and style (`style`, only on models that support it), and character | yes: public `https://` URLs and Files API URIs; inline base64 up to a 20 MB request | `POST https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`, `responseModalities: ["IMAGE"]` | `x-goog-api-key` disguise (M1) | images returned |
| `openai` | ✓ | up to 16 per request through `POST /v1/images/edits`; no roles | yes: `image_url` with a URL or data URL, or a `file_id` | `POST {endpoint}/images/generations` (no references), `POST {endpoint}/images/edits` (references) | bearer (M1) | images returned |

## R5

- **Chosen:** `google` (Gemini image models on the Gemini API) and `openai` (GPT image models)
  \[PROP\]. Both authenticate with API keys under 5.4 M1, accept reference images inline or as URLs,
  and return inline base64. Neither depends on Vertex.
- **Roles.** Neither API has a role field per image. Gemini documents role-specific limits (object,
  character and style references) and takes the role from the prompt; OpenAI has no roles. Both
  adapters write each reference's role into the prompt text, next to the image's position, in one
  fixed form so every SDK sends the same request \[PROP\]: `Image {n} is a {role} reference.`
- **URLs** (plan D8). Both accept URLs, so the library passes a URL reference through and fetches
  bytes only for a URL scheme the provider cannot read.
- **Results** are inline bytes (`b64_json`, `inline_data`); the library never returns a provider URL.
- **Not chosen:** Imagen customization (subject and style reference types) needs Vertex service-account
  auth and is being retired in favour of Gemini image models; Stability (multipart bytes only, one
  reference), BFL FLUX (asynchronous polling, result URLs that expire in 10 minutes), xAI (no roles,
  temporary URLs) and Bedrock Nova Canvas (SigV4, variation rather than references) are possible later
  branches. OpenAI's `gpt-image-1` was shut down on 2026-10-23; models are named by the value, so this
  needs no contract change.

## R6

| Provider | STT streaming | TTS streaming | Source of turn detection |
| -- | -- | -- | -- |
| Deepgram | WebSocket `/v1/listen` (Nova) and `/v2/listen` (Flux); `mulaw` 8000 Hz and `linear16` in | WebSocket `/v1/speak`; `mulaw` 8000 or 16000 Hz, `alaw`, `linear16` out | Nova: `speech_final` (after `endpointing` ms of silence, default 10) and `UtteranceEnd` (`utterance_end_ms`); Flux: model-based `EndOfTurn` (`eot_threshold`, `eot_timeout_ms`) and `EagerEndOfTurn` |
| ElevenLabs | — (not an STT provider here) | WebSocket `stream-input`; `ulaw_8000`, `pcm_*` | — |
| AWS | not possible under 5.4 (chunk signing or presigned URL) | per-item REST | — |
| Azure | not possible (undocumented protocol) | — (not a TTS provider here) | — |
| Google | not possible (gRPC only) | per-item REST (gRPC only for streaming) | — |
| OpenAI (Whisper) | not supported by `whisper-1` | — | — |

So Deepgram is the only provider on the minimum list that streams STT, and the voice package takes
turn detection from Deepgram's signals, with its own silence timer only for a provider without one
([voice.md](voice.md#turns)). Done means' "Streaming speech-to-text, with a provider that streams"
uses Deepgram.

## G4

**The gate:** one live Hugging Face `text-ranking` call with a real token, before the schema freeze.
If the hosted task is unusable, hosted rerank fails with an explicit `AiError` (`unsupported`) as in
1.6, and TEI endpoints stay supported.

**Outcome (2026-11-10): supported.** Hosted `text-ranking` works and returns the shape the adapter
expects. The fallback (hosted rerank as `unsupported`) is not needed.

The exchange, with the token redacted (it is never committed):

```http
POST https://router.huggingface.co/hf-inference/models/cross-encoder/ms-marco-MiniLM-L6-v2
Authorization: Bearer <redacted>
content-type: application/json

{"inputs":{"query":"capital of France","texts":["Berlin is in Germany.","Paris is the capital of France.","France is in Europe."]}}
```

```http
HTTP/2 200
content-type: application/json
x-inference-provider: hf-inference

[{"score":8.818082809448242,"index":1},{"score":-2.7613704204559326,"index":2},{"score":-9.78369140625,"index":0}]
```

| Model | Hub task mapping | Result |
| -- | -- | -- |
| `cross-encoder/ms-marco-MiniLM-L6-v2` | `text-ranking` | 200, Paris (index 1) first, 4.3 s cold and 0.2 s warm |
| `Qwen/Qwen3-Reranker-0.6B` | `text-ranking` | 200, the same order, 18 s cold |
| `BAAI/bge-reranker-v2-m3` | `text-classification` | 400: `TextClassificationPipeline.__call__() missing 1 required positional argument: 'inputs'` |

What this fixes in the contract:

- **Shape.** One `{index, score}` per text, sorted by score, best first. `score` is the model's raw
  logit: unbounded and possibly negative, not a probability in [0, 1]. `ranked.score` is the
  provider's score as reported, so callers compare scores only within one provider and model.
- **Usage.** Neither the body nor the headers carry usage, so the no-usage rule applies: the call
  commits ⌈documents × weight⌉ ([usage-and-quota.md](usage-and-quota.md)).
- **Models.** Only models whose Hub mapping for `hf-inference` is `text-ranking` accept this body. A
  `rerank-model` mapped to another task (for example `BAAI/bge-reranker-v2-m3`) gets HTTP 400, which
  maps to `invalid-request`. The library does not check the mapping before sending.
- **Latency.** A cold model can take tens of seconds; the adapter's timeout must allow it.
- **Billing.** The account needs Inference Providers credits: without them every call answers HTTP
  402 ("Purchase pre-paid credits…"), which maps to `authentication`
  ([errors.md](errors.md#kinds)).

Self-hosted TEI (`POST {endpoint}/rerank` with `{"query","texts"}`, returning `[{index, score}]`) is
supported as well; GOL-1355 records it.

## The STT accuracy threshold

Done means requires that streaming speech-to-text, with a provider that streams, transcribes a fixed
audio fixture within an accuracy threshold.

- **Threshold** \[PROP\]: word error rate (WER) at most **0.10** for the fixture as recorded (16-bit
  linear PCM, 16000 Hz, mono), and at most **0.15** for its telephony form (mu-law, 8000 Hz, mono,
  converted with the [voice.md](voice.md#audio-format) conversions).
- **Fixture**: one English speech recording of 20–40 seconds, clean read speech by one speaker, with
  no numbers or proper nouns that have several spellings, under a licence that permits committing it
  (for example an utterance from LibriSpeech `test-clean`, CC BY 4.0), with its reference transcript.
  GOL-1357 adds it to the corpus.
- **Measure**: normalize the reference and the hypothesis (the concatenated `final` segments): lowercase,
  replace hyphens with spaces, remove every other punctuation character except apostrophes, collapse
  whitespace. WER = (substitutions + deletions + insertions) / reference words, from a word-level
  Levenshtein alignment.
- **When**: at recording time (GOL-1357 streams the fixture to the real provider in both forms; the
  recorder fails above the threshold, so a bad recording never enters the corpus) and in the manual
  real-model check before each release. CI replays the recorded transcript and computes the same WER,
  so a parser that drops or reorders segments fails there too.
