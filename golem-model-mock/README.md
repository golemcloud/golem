# golem-model-mock

A scripted, in-process OpenAI-compatible server for Golem tests. No model account,
real credential or external process is needed. Each `ModelMock` binds its own
`127.0.0.1:0` listener and owns its script. It serves
`POST /v1/chat/completions` only.

Integration tests import `golem_test_framework::model_mock`; CLI integration tests
import `golem_model_mock` directly. The crate does not depend on Golem services.

## Rust API

```rust
use golem_model_mock::{ChatResponse, Expectations, ModelMock, Script, Step};

async fn example() -> anyhow::Result<()> {
    let mock = ModelMock::start(Script {
        steps: vec![Step {
            expect: Expectations {
                model: Some("scripted-model".into()),
                tool_names: Some(vec![]),
                last_message_role: Some("user".into()),
                authorization: Some("Bearer mock-secret".into()),
                ..Default::default()
            },
            response: ChatResponse::Text { text: "Hello!".into() },
        }],
    }).await?;

    // Point the provider client at this API base, then await all its calls.
    let endpoint = format!("{}/v1", mock.base_url());
    // ... exercise the client ...
    mock.verify()?;
    Ok(())
}
```

`Script.steps` is ordered. Each valid JSON chat request consumes the next step;
a mismatch is permanent, even if later requests match. A mismatch, malformed
request, wrong path or method, or call after the last step returns HTTP 400 with an OpenAI-style
error. The error names the one-based step and the differing field. It never
includes the expected or received credential, or the request body.

`verify()` returns an error for recorded failures and unrequested steps. It does
not clear failures. Drop aborts the server and panics if verification fails, so
forgetting `verify()` cannot hide an incomplete conversation. During panic
unwinding, drop only aborts, to avoid a double panic. Keep the fixture until all
client calls and stream consumers finish; do not leak it or move it to an
unobserved task. Requests have the axum-style 2 MiB body bound.

Expectations are optional:

| Field | Check |
| --- | --- |
| `model` | Exact model string |
| `tool_names` | Exact offered function-tool names, ignoring order but counting duplicates; `[]` requires no tools |
| `last_message_role` | Role of the final message |
| `last_tool_call_id` | `tool_call_id` of the final message |
| `authorization` | Exactly one `Authorization` header with the exact value, including `Bearer ` |

The fixture only checks the named expectations. It does not validate every
OpenAI option, tool schema, or conversation message. Set expectations on each
step that matters to the test. `model` must always be a string. The request's
`stream` flag must agree with the response kind.

## JSON scripts

`Script::from_json(&std::fs::read_to_string(path)?)` loads a file into the same
types used by the Rust API. Unknown fields are rejected, including expectation
typos. Parse errors give a line and column without echoing the script. Use only
test credentials in committed scripts.

[examples/tool-loop.json](examples/tool-loop.json) is a complete two-step
conversation: request a `purchase` call, then return text after its tool result.
The Rust/file equivalence test executes both forms through `async-openai`.

```json
{
  "steps": [{
    "expect": {"model": "scripted-model", "last_message_role": "user"},
    "response": {"type": "text", "text": "Hello!"}
  }]
}
```

Three response kinds are supported:

- `{"type":"text","text":"Hello!"}` produces a `chat.completion` assistant
  message with `finish_reason: "stop"`.
- `{"type":"tool_calls","calls":[{"id":"call-1","name":"purchase","arguments":"{\"quantity\":3}"}]}`
  produces function `tool_calls` with `finish_reason: "tool_calls"`. Arguments
  are a **string containing JSON**, not a JSON object.
- `{"type":"stream","deltas":[{"content":"Hel"},{"content":"lo!"}]}`
  produces one `chat.completion.chunk` SSE event per delta, preceded by an
  assistant-role event and followed by a finish event and `data: [DONE]`.

For streamed tool calls, use `Delta.tool_calls` (`tool_calls` in JSON). Each entry
has `index`, optional `id` and `name`, and an `arguments` string fragment. Set the
ID and function name on the first fragment and use the same index to continue
it. [examples/streamed-tools.json](examples/streamed-tools.json) interleaves two
calls and splits arguments inside their keys. A stream containing tool deltas
ends with `finish_reason: "tool_calls"`; other streams end with `"stop"`.
The fixture preserves scripted fragments. It does not impose timing or TCP
packet boundaries. Clients must parse SSE independently of network read sizes.

IDs (`chatcmpl-mock-<step>`) and timestamps (`created: 0`) are deterministic.
There are no usage reports, fault injection, other provider protocols, speech,
embedding or reranking endpoints. Later provider work extends this crate.

## Golem CI configuration (5.5 and 3.3)

Use `provider: openai`, `format!("{}/v1", mock.base_url())` as `endpoint`, and any
scripted model name. The provider appends `/chat/completions`, as in GOL-1351's
OpenAI wire contract and ordinary OpenAI clients. `base_url()` is the server's
origin, without `/v1`; the provider's API base includes `/v1`. Substitute the
actual assigned origin for `http://127.0.0.1:43127` below:

```yaml
provider: openai
endpoint: http://127.0.0.1:43127/v1
model: scripted-model
credential: { $secret: credentials.model-mock }
```

The credential is a Golem secret, not a string key in agent code. Give it a
test-only value such as `mock-secret`, and expect `Bearer mock-secret` at the
mock. In stand-in tests, the agent sends the stand-in: **Golem**, not the mock,
must substitute the actual secret before the HTTP request reaches the fixture.
An authorization expectation rejects a `golem-standin-…` value that was not
substituted. The mock does not implement substitution.

The secret's destinations must contain the exact plain HTTP origin, including
the assigned port (5.4 OA1). `localhost` is not the listener's `127.0.0.1` host;
do not use an HTTPS destination or a path such as `/v1`:

```yaml
config:
  ci:
    credentials:
      model-mock:
        $kind: secret
        $destinations: [http://127.0.0.1:43127]
    assistant-model:
      $kind: shared
      $default:
        provider: openai
        endpoint: http://127.0.0.1:43127/v1
        model: scripted-model
        credential: { $secret: credentials.model-mock }
```

This catalog example follows the proposed 1.4 spelling in the GOL-1351 contract
(PR #4395), not the older separate `secrets:` catalog in the initial 3.1 spec.
The config/secret tickets own the final manifest grammar; this fixture does not
depend on them or on that PR merging.

The calling principal's explicit card must grant **network access to host
`127.0.0.1`** and the assigned mock port (5.3 D28); a grant for a public provider
or `localhost` does not cover it. It must also grant `secret : hold` for
`credentials.model-mock` and `reserve` for the model's quota resource (or the
`golem-ai.llm` default meter when no quota is given). Use the authority API of the
test suite to construct those grants. Start the fixture before filling the
endpoint, destinations and card so they all use the same assigned port. The
executor and fixture must share the loopback network namespace; a separate
container cannot reach this listener through its own loopback address.

Never call a real model in CI. This fixture needs no provider account and does
not load credentials from the environment.

## Verification

```shell
cargo test -p golem-model-mock --lib -- --report-time
cargo fmt -p golem-model-mock -- --check
cargo clippy -p golem-model-mock --all-targets -- -D warnings
```

The wire contract comes from the OpenAI API reference:
[Chat Completions](https://platform.openai.com/docs/api-reference/chat) and
[streaming tool calls](https://platform.openai.com/docs/guides/function-calling#streaming).
Hand-written SSE transcripts in `tests/goldenfiles/` check framing, deltas and
termination; `async-openai` independently parses plain and streamed responses.
