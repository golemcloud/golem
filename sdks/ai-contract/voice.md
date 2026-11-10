# The voice contract

**Layer:** binding: turn detection, barge-in, playback, transcript lines and recovery as behaviour.
Reference model: the session API and its records.

The voice package turns an audio stream into caller turns and spoken replies. It is built from a
`Transcription` field, a reply function (often a `streamWith` run) and a `Speech` field, with turn
detection, barge-in, filler and explicit media recovery. It ships in every SDK, on that SDK's Golem AI
library (X12). Its API is \[PROP\]; every name on this page is \[PROP\].

The package stays provider-neutral. Provider framing (for example Twilio Media Streams), DTMF and call
control belong to adapters and tools. Transcripts are agent state: the package produces transcript
lines and the agent keeps them.

## Session

> The types on this page use the contract's WIT-like notation ([README.md § Notation](README.md#notation)). It is notation only: Golem AI defines no WIT interfaces, and each SDK writes these types by hand in its own idiom. Unless the page's Layer line says otherwise, they are the reference model ([README.md § Layers](README.md#layers)): each SDK exposes their information in its own API design.

```text
session(config: voice-config) -> voice-session

record voice-config {
  hearing: Transcription,                  // a Shared field or a value
  speaking: Speech,                        // a Shared field or a value
  reply: func(turn: turn, signal) -> reply,
  format: audio-formats,                   // the session's input and output formats
  seed: option<voice-seed>,
  voice: option<string>,                   // the Speech provider's voice ID
  language: option<string>,
  filler: option<filler>,
  recovery-phrase: string,                 // spoken after a crash during speech
  turn-silence-ms: u32,                    // default 700; used only when the provider has no turn signal
  barge-in-min-speech-ms: u32,             // default 200
}

variant reply {
  text(string),
  stream(stream<string>),                  // for example the text projection of a streamWith run
}

record turn {
  id: string,                              // unique within the session
  attempt: string,                         // the response attempt ID; a new one after each barge-in
  text: string,                            // the caller's final transcript for this turn
  lines: list<transcript-line>,            // the transcript so far, for the reply's context
}

voice-session.run(input: stream<voice-input>) -> stream<voice-output>
```

- One session per media invocation. All its work runs inside that invocation: the STT session, the
  model turns, filler timers and playback.
- A session opens when `run` starts and closes when the input ends, the caller's cancellation arrives,
  or the output is dropped. Closing ends the STT and TTS sessions and the in-flight reply.
- Each turn is one call of `reply`. The usual reply is one `streamWith` with local tools that are
  closures over agent state.
- The package opens one tracing span per turn (3.3 D6).
- On a serial agent, a session keeps the agent busy while it waits for input; other calls stay
  pending during the call (1.2).

## Seeding from agent state

```text
record voice-seed {
  lines: list<transcript-line>,            // the transcript kept in agent state
  played: list<string>,                    // sentence IDs confirmed as played
  pending: option<string>,                 // the sentence ID that was being spoken, if any
}
```

A session started with a seed continues the same conversation: transcript lines keep their order and
IDs, played sentences are never spoken again, and a `pending` sentence triggers media recovery. A new
media leg and a media invocation restarted after a crash both start from the seed (3.3 D13).

## Audio format

```text
record audio-formats { input: audio-format, output: audio-format }

record audio-format {
  encoding: encoding,
  sample-rate: u32,
  channels: u8,                            // 1 for telephony
}

enum encoding { mulaw, alaw, pcm16 }       // pcm16: 16-bit signed little-endian linear PCM
```

The session declares its input and output formats; for telephony both are mu-law, 8000 Hz, mono. It
converts only where a provider needs another format (3.3 D4):

| Conversion | Algorithm |
| -- | -- |
| mu-law ↔ PCM16, A-law ↔ PCM16 | ITU-T G.711 tables |
| Resampling between 8000, 16000, 22050, 24000 and 48000 Hz | linear interpolation over PCM16 samples \[PROP\]; fixed so that every SDK produces the same bytes for the voice fixtures |

With Deepgram (`mulaw` 8000 Hz in and out) and ElevenLabs (`ulaw_8000` out), telephony needs no
conversion.

## Input

```text
variant voice-input {
  audio(list<u8>),                         // frames in format.input
  played(string),                          // the adapter confirmed that the audio up to this mark was played
  cleared(string),                         // the audio up to this mark was dropped by a clear
  end,                                     // the media leg ended
}
```

`played` and `cleared` carry the mark IDs the session emitted. A mark that comes back after a `clear`
is `cleared`, never `played`; the adapter decides which (for Twilio, marks returned after a `clear`
message).

## Output

```text
variant voice-output {
  audio(list<u8>),                         // frames in format.output
  mark(string),                            // the end of one sentence's audio; the ID is the sentence ID
  clear,                                   // drop all buffered audio now (barge-in)
  line(transcript-line),                   // a new or updated transcript line to keep in agent state
  turn-started(turn-ref),
  turn-ended(turn-ref),
}

record turn-ref { id: string, attempt: string }

record transcript-line {
  id: string,
  role: line-role,                         // caller or agent
  text: string,                            // agent lines hold only sentences confirmed as played
  turn: string,
  attempt: option<string>,                 // agent lines only
  status: line-status,
  start-ms: option<u64>,                   // caller lines: from the transcript segments
  end-ms: option<u64>,
}

enum line-role { caller, agent }
enum line-status { final, interrupted }    // interrupted: an agent line cut by barge-in
```

- There is one `mark` after each sentence's audio. The session splits reply text into sentences at
  `.`, `!`, `?` and line breaks \[PROP\], and at 300 characters when a sentence is longer.
- An agent line grows as `played` marks arrive. When barge-in cuts it, the line ends with status
  `interrupted` and holds only the played sentences.
- A caller line is emitted when the turn's transcript is final.

## Turns

- **Turn detection.** The end of a caller turn comes from the provider's own signal where it has one:
  Deepgram Flux `EndOfTurn`, or Deepgram Nova `speech_final` followed by `UtteranceEnd`
  ([providers.md](providers.md#r6)). A provider without a turn signal falls back to the package's
  silence timer: `turn-silence-ms` without a new final segment ends the turn. The session uses the
  provider's signal whenever it exists.
- **Response attempts.** Each reply gets a response attempt ID. A barge-in ends the attempt: the
  session cancels the reply through its signal, stops TTS, emits `clear`, and the next caller turn
  starts a new attempt.
- **Barge-in.** Caller speech of at least `barge-in-min-speech-ms` while agent audio is playing (sent
  and not yet confirmed `played`) is a barge-in. The 3.3 budget for the reaction is 300 ms.
- **Filler.** With `filler`, when a reply produces no text within `filler.after-ms` (default 1500
  ms), the session speaks one of `filler.phrases`; it never repeats one in a turn. Filler sentences
  have marks and appear in the agent line only when played.

```text
record filler { after-ms: u32, phrases: list<string> }
```

## Recovery

- **Speech.** After a crash during agent speech (a sentence was sent and not confirmed played, the
  seed's `pending`), the session plays a short silence (300 ms \[PROP\]), then `recovery-phrase`, then
  the interrupted sentence again from its first word. Golem never sends corrupted audio; audio in
  flight at the crash may be cut off or partly repeated on the caller's side.
- **Text.** A reply stream interrupted by a crash follows Rule 10's continuation
  ([recovery.md](recovery.md#text-rule-10)). Sentences already confirmed played are not spoken again.
- **STT.** After `session-lost`, the session reconnects and resends the kept audio (the audio since
  the last final segment). After a crash, the transcript follows Rule 10's shape
  ([recovery.md](recovery.md#transcripts)).
- **TTS.** A `media-interrupted` from `speakStream` is handled by the speech rule above.

## Which providers stream

| Capability | Streams | Does not stream |
| -- | -- | -- |
| Speech-to-text (`transcribeStream`) | Deepgram (Nova over `wss://api.deepgram.com/v1/listen`, Flux over `/v2/listen`) | AWS, Azure, Google, Whisper: `unsupported` ([providers.md](providers.md#matrix)) |
| Text-to-speech (`speakStream`) | Deepgram (`wss://api.deepgram.com/v1/speak`), ElevenLabs (`stream-input`) | AWS Polly and Google: one REST request per input item ([providers.md](providers.md#r6)) |

A voice session needs a `Transcription` value whose provider streams; any other value fails the
session at start with `unsupported`. Any `Speech` value works; one without a streaming protocol adds
the latency of one request per sentence.

## Latency

The 3.3 budget is 1.0 s at p50 from the end of caller speech to the first agent audio, 300 ms for
the barge-in reaction, and at most 150 ms at p95 for Golem's share. The package is measured with the
mock server against 7.1's synthetic media producer (GOL-1220).
