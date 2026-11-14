# `AiError`

**Layer:** binding: the kinds, when each one occurs, whether it falls back, and what never becomes
an `AiError`. Reference model: the `ai-error` record and its causes.

Every Golem AI failure is one `AiError` family with shared kinds (Rule 4; Proposed: "One `AiError`
family"). Each SDK spells each kind once, in its idiom: a Rust enum variant, a TypeScript string
literal union, a Go sentinel compared with `errors.Is`, and so on. The kind names are \[PROP\].

Cancellation is not an `AiError`. A cancelled operation ends in the SDK's cancellation form
([interfaces.md](interfaces.md#cancellation)) and the library never turns it into an error value or
a partial success (1.2 O7).

## Shape

> The types on this page use the contract's WIT-like notation ([README.md § Notation](README.md#notation)). It is notation only: Golem AI defines no WIT interfaces, and each SDK writes these types by hand in its own idiom. Unless the page's Layer line says otherwise, they are the reference model ([README.md § Layers](README.md#layers)): each SDK exposes their information in its own API design.

```text
record ai-error {
  kind: ai-error-kind,
  message: string,                  // safe for logs: never a credential, stand-in or request body
  model: option<model-ref>,         // the model whose call failed, when one was chosen
  provider-status: option<u16>,     // the provider's HTTP status, when there was a response
  provider-code: option<string>,    // the provider's own error code, when it sent one
  cause: option<cause>,
  attempts: list<attempt>,          // the fallback members tried before this error (operations.md#fallback)
}

variant cause {
  egress-auth(egress-auth-error),   // 5.4's EgressAuthError, unchanged
  foreign(foreign-error),           // a wrapper's own failure, for example Memory's
  transport(string),                // the SDK's HTTP or WebSocket error, as text without secrets
}
```

`egress-auth-error` is the `EgressAuthError` of 5.4 (GOL-1144), with its kind: `destination`,
`hold`, `network`, `deleted-revision`, `consent-pending`, `body-too-large`, `token-endpoint`. The
library detects it as the cause of the transport error: the `golem-egress-auth: <kind>: …` prefix on
wasi:http sends, and the typed `egress-auth` case on `golem:websocket` connect and `golem:secrets`.

`foreign-error` keeps the wrapper's own error value in the SDK's idiom (a boxed error in Rust, the
thrown value in TypeScript, an `error` that `errors.As` finds in Go, a `Cause` in Effect). The library
never inspects it and never drops it.

## Kinds

| Kind \[PROP\] | When | Sent to the provider? | Fallback |
| -- | -- | -- | -- |
| `unsupported` | The serving provider cannot make the call (Rule 4), for example `transcribeStream` on AWS, `rerank` on OpenAI, `countTokens` outside Anthropic, or `rerank` on a value without a reranking model | No | Yes |
| `unavailable` | The provider could not serve the call: a connection failure, a timeout, HTTP 408, 429, 500, 502, 503, 504 or 529 after the ambient retries, or a stream that broke and could not be continued | Yes (or attempted) | Yes |
| `quota-rejected` | The reservation was refused with `failed-reservation`: the quota resource's policy is `reject` and its allocation is exhausted. Carries the estimated wait | No | Yes |
| `reserve-denied` | The calling invocation's effective authority has no `reserve` grant on the quota resource (Rule 12; plan G5) | No | No |
| `authentication` | The provider refused the credential or the account behind it (HTTP 401, 402 or 403; 402 is an account without credits) | Yes | No |
| `egress-auth` | The host refused to place the credential: the cause is the 5.4 `EgressAuthError`. Inside an agent-hosted tool or entity invocation this is the result until GOL-1146 lands | No | No |
| `invalid-request` | The provider rejected the request as malformed or over its limits (HTTP 400, 404, 413, 422), or the library found that before sending (for example a media type the provider does not accept) | Sometimes | No |
| `invalid-tools` | Rule 8's run-start check rejected the toolset: duplicate names, a name or schema that a model the run may use cannot accept, or an argument that projects to a reserved name. The message names the tool | No | No |
| `invalid-output` | The provider's response could not be used: structured output that does not decode against the schema, or `embed` with a vector count that is not one per input | Yes | No |
| `media-interrupted` | A media call was interrupted after its output was exposed and cannot be resumed (Rule 11). Carries what was delivered | Yes | No |
| `foreign` | A wrapper raised its own failure through the family; the cause is `foreign` | Depends | No |

Kinds are disjoint: each failure has exactly one. A `throttle` or `terminate` enforcement does not
produce an `AiError` (see below).

### Refused reservations

A model call whose reservation is refused is never sent \[PROP\] (Rule 12; plan G6b). How the call
continues depends on the refusal:

| Refusal | Outcome |
| -- | -- |
| No `reserve` grant | `reserve-denied`. No fallback member is tried. |
| `reject` (`failed-reservation`) | `quota-rejected`. The next fallback member is tried; this is the spec's "quota rejection". |
| `throttle` | No error. The reservation waits: the host suspends the agent and resumes it when capacity is expected, and the call then continues on the same member. |
| `terminate` | No error. The host ends the holder (the agent); no fallback runs. |

### Fallback triggers

The fallback triggers \[PROP\] are `unavailable`, `quota-rejected` and `unsupported`: unavailability,
quota rejection and calls the serving provider cannot make. Authentication failures and invalid
requests never trigger a fallback, and neither do `reserve-denied`, `egress-auth`, `invalid-tools`,
`invalid-output`, `media-interrupted` or `foreign`. A trigger moves the call to the next member only
before the call exposed anything ([operations.md](operations.md#fallback)).

### Errors from tools inside a tool loop

A bound tool's failure, denial or admission failure, and a local tool's declared tool error, go back
to the model as a tool result and do not fail the run (Rule 7; [tool-loop.md](tool-loop.md)). A local
tool handler that fails in any other way ends the run with `foreign`, whose cause is the handler's
error. Cancellation inside a tool propagates as cancellation.

## Mapping provider responses

Each provider adapter maps its responses to these kinds and keeps `provider-status` and
`provider-code`. The fixture corpus holds one recorded error response per kind that a provider can
produce, and every SDK's unit tests check the mapping against it. `message` never contains a
credential, a stand-in, a request body or response text that could echo one. A provider error body
is reduced to its documented error code and message fields.
