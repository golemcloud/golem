# Recovery

**Layer:** binding.

This page states Rules 9–11 as behaviour a caller can observe. It applies in a durable caller. In an
ephemeral caller a crash fails the invocation, and nothing here is promised.

The mechanisms are the host's: recorded P3 HTTP sends and response chunks, the
`http-response-incomplete` error when a send has no safe resend, `golem:websocket`'s `session-lost`
under `report-connection-loss` (the default after GOL-1127), and recorded quota calls. The libraries
add only what is below.

## Model calls are never sent again (Rule 9)

- A provider request whose response was fully received before a crash is never sent again: replay
  returns its recorded response.
- A request in flight at the crash is restarted or continued, as below. The provider may bill the
  interrupted request as well.
- A rerun of an idempotent command repeats its model calls (2.1 Rule 4). Each sent request counts
  against the `Quota`.
- Completed tool calls are recorded invocations; they never run again.

## Text (Rule 10)

| Interrupted | Caller observes |
| -- | -- |
| `stream` or `streamWith`, after some `text-delta` events were delivered | The delivered events stay delivered and are not replayed to the caller. The stream goes on with the rest of the answer, from a continuation request. The model may repeat or omit content at the seam. |
| `run` or `runWith` | One result. The step that was interrupted restarts internally; nothing was exposed. |
| A step whose interrupted part is an incomplete tool call or a partial structured value | That part restarts internally. It was never exposed. |
| Between steps | Completed steps and their tool calls are kept; the loop continues with the next step. |

**The continuation request** has the original request of the step, and the text received so far as
context. Where the provider accepts a trailing assistant message as a prefix (`anthropic`, `ollama`),
the prefix is sent as that message; otherwise the request adds the continuation instruction
\[PROP\]. Its exact text is in the fixture corpus, so every SDK sends the same bytes. The text below
is a proposal; GOL-1353 may change it when it writes the corpus:

> You were asked the same question previously, but the response was interrupted before completion.
> Please continue your response from where you left off. Do not include the part of the response that
> was already seen.

followed by the original question and the partial response. The continuation's text follows the prefix in the same step: one
`step-start`, one `step-end`, and `messages` hold one assistant message with the whole text, never the
continuation instruction. The step's usage is the continuation's reported usage; the interrupted
request reported none.

The same continuation is used when a stream breaks without a crash (a connection reset mid-answer):
the ambient retry rules decide whether the library tries again, and a retry after delivered text is a
continuation, never a resend from the start. If the retries are exhausted, the stream ends with
`unavailable` after the delivered events, and no fallback member is tried
([operations.md](operations.md#fallback)).

## Media (Rule 11)

| Call | Output exposed before the crash? | Caller observes |
| -- | -- | -- |
| `transcribe`, `speak`, `generate`, `embed`, `rerank`, `countTokens` | Never: the result is returned whole | One result. The call restarts internally. |
| `speakStream` | No audio delivered yet | One stream. The call restarts internally. |
| `speakStream` | Audio delivered | The stream fails with `media-interrupted`, carrying what was delivered: the audio already delivered and the index of the last input item whose audio was complete. It is not resumed, because a provider speech stream cannot be continued without duplicated or cut audio. |
| `transcribeStream` | Segments delivered | A text stream; see [Transcripts](#transcripts). |

Corrupt or duplicated media is never returned. A partial image, a truncated audio body or audio that
would repeat delivered audio is never exposed. A WebSocket that reports `session-lost` after
reconstruction is handled by the same table.

Recovering the conversation after `media-interrupted` is the caller's policy. The voice package does
it with a short silence, a recovery phrase and the interrupted sentence again
([voice.md](voice.md#recovery)).

## Transcripts

A transcript stream is a text stream, so it follows Rule 10's shape, not Rule 11's (plan G7):

- `final` segments already delivered are kept and are never emitted again.
- `interim` hypotheses that were not final are dropped and start again internally.
- A new provider session receives the audio since the last delivered `final` segment: the kept audio,
  rebuilt during replay from the input stream's consumer journal. Where the provider accepts them, it
  also gets the keywords of the call.
- Segment times stay on the original timeline: the library adds the offset of the kept audio to the
  new session's times.
- Words at the seam may repeat or be missing, as Rule 10 allows.
- Batch `transcribe` exposes nothing until it finishes, so it restarts internally.

## Quota across recovery

Reservations and commits are recorded host calls, so replay never counts a completed call again
(Rule 12). The interrupted request was sent and never reported usage, so it commits its reserved
amount (Rule 12: "unless an estimate was reserved for it" \[PROP\]). The continuation or restarted
request is a new provider request: it reserves and commits on its own, because it was sent too (Rule
9). See [usage-and-quota.md](usage-and-quota.md#commit-after-the-call).
