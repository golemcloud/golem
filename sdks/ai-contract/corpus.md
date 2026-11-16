# Provider fixture corpus

The root-workspace `golem-ai-contract` tool records provider exchanges. Tests and corpus checks
never need a credential or a network connection. The recorder is manual and refuses to run when
`CI` is set. It reads `OPENAI_API_KEY` from the environment and sends only to `api.openai.com`,
without redirects. It never writes raw captures to disk.

## Format

A recording directory contains `index.json`, one `<case-id>.json` recording per case, and one
`<case-id>.body` response per case. The index declares `caseIds`, recording filenames in `cases`,
and `operations` with `provider`, `operation` and `streaming`. An operation without a streaming
protocol declares `streaming: false`; it does not invent an SSE response.

Each recording has `id`, `provider`, `operation`, `provenance`, `request` and `response`.
The request has `method`, `url`, `headers` and its JSON `body`. The response has `status`, `headers`,
a sibling `body` filename and a `format`: `json`, `sse` or `malformed`. SSE bodies retain line
framing, CRLF where present, and the terminal sentinel. JSON values inside data lines are
normalized, not replaced with a parsed event list.
The directory's Git attributes keep JSON sources at LF and disable line-ending conversion for
`.body` files, so checkout does not change the recorded wire bytes.

`provenance: live` means a response came from the real provider. `fault-injection` means a
deliberately constructed failure, not evidence of provider success. The OpenAI recorder captures
plain and streamed chat, structured output and function calls, embeddings with two separate inputs,
and a live HTTP 401 per operation. It adds fault injections for HTTP 429 with `Retry-After: 17`,
HTTP 503 and malformed JSON. It does not exhaust an account or cause a real provider outage.

The checker rejects missing plain, stream or error coverage, unlisted files, duplicate IDs,
undeclared operations, unsafe paths, and non-normalized credentials or metadata.
Successful OpenAI streams must contain response data followed by a single terminal `[DONE]`,
with no response data after it. Both LF and CRLF framing are accepted.

`openai-faults.json` holds the hand-written 429, 503 and malformed-body cases and their expected
error kinds from `errors.md`. These are fault injections, not live recordings. The recorder uses
these same cases; it does not keep a second set of error bodies in Rust source. The malformed
successful response is `invalid-output`, because the provider's response cannot be used.

## Shared files and offline checks

`corpus-index.json` lists the shared JSON assets in `files` and the directories of complete live
recording sets in `recordingSets`. A recording set must pass the coverage and normalization check
before it can enter the corpus. Add canonical schema files to `files` when they are available.
Any unlisted `.json` or `.body` file under this directory makes the check fail.

```console
cargo make check-ai-contract
```

The check runs in `unit-tests-and-checks`. It never writes files, calls a provider or reads a key.
Each fixture asset declares its case IDs; a mismatch fails the check. SDK provider tests should
read the shared files in this directory, preserving raw SSE framing, CRLF and `[DONE]`. There are
no generated SDK copies. Adding provider replay tests and any required test-runner file access
belongs to the SDK provider implementations.

The checked-in assets contain token estimates, synthetic faults and the `openai` recording set.
That set was captured once with `gpt-4.1-mini` and `text-embedding-3-small`: seven successful
requests and four authentication failures using an invalid key. The successful requests used
254 input tokens and 34 output tokens in total, including the embedding inputs. Plain and streamed
structured output return the requested Paris/France record; tool calls select `weather` with
Budapest as the argument; embeddings preserve two input positions with distinct eight-element
vectors. Streams include the usage chunk and terminal sentinel. No canonical capability schemas
exist yet. Nested-secret support and the capability types that require it are separate from this
corpus tool.

## Normalization

Credential header values and `key`, `api_key`, `api-key` and `access_token` query values become
`<credential>`. The recorder also scrubs its known credential from response bodies, because an
authentication error can echo it. URL user info is rejected. Headers use lowercase names. Dates
become `<date>`, request IDs become `<id>`, and JSON `system_fingerprint` becomes `<fingerprint>`.
JSON response IDs become `<id:1>`, `<id:2>`, etc., consistently across all chunks of one response,
so different tool calls remain distinct. Model names, content, usage and numeric `Retry-After`
values remain unchanged. Normalizing the result again produces the same bytes.

## Manual recording

With `OPENAI_API_KEY` already available in the environment:

```console
cargo run -p golem-ai-contract -- record-openai /tmp/openai-recordings
cargo run -p golem-ai-contract -- check /tmp/openai-recordings
```

The output directory must not exist. Review every request against the provider reference and keep
the live success responses as evidence before copying the directory into the contract. The current
request templates follow OpenAI's Chat Completions API (`response_format.json_schema`, function
`tool_choice`, and `stream_options.include_usage`) and embeddings API (`input` as an array and
`dimensions`, not `dimension`). Model names can be selected with `--model` and `--embedding-model`.
After review, copy the directory under `sdks/ai-contract/`, add its relative path to `recordingSets`
in `corpus-index.json`, and run `cargo make check-ai-contract`. No test needs the recording key.

`estimate-tokens.json` is hand-calculated from `operations.md`: count Unicode scalar values and
round up after dividing by 4 or 3.5. Four CJK characters and four astral emoji deliberately have the
same expected count as four ASCII characters. Combining marks count separately, not as graphemes.
