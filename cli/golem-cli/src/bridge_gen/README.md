# Guest streaming bridges

Guest bridges are compiled into a Golem component and invoke another component through
awaited Wasm-RPC. They use the guest SDK's native `AgentStream`, not the external bridge
runtime's stream sessions, tokens, or registration contexts.

## Generated surface

- A typed schema stream becomes `AgentStream<T>` in Rust and TypeScript and
  `AgentStream[T]` in Scala and MoonBit.
- Stream-bearing methods expose awaited invocation and the language's existing awaited
  cancellation surface. Trigger and schedule variants are not generated, including for
  methods whose only stream is in the output.
- Streams can occur inside records, variants, results, options, collections, recursive
  references, and other streams. Their number and position are determined by the value;
  there is no fixed list of stream slots.
- An untyped stream (`Stream(None)`) cannot supply a native item codec and is rejected.
- Non-streaming methods retain their existing invocation variants.

## Item codecs and producers

Item conversion is derived from the particular source schema, not solely from its generated
language type. For example, a Rust `Vec<T>` can represent a list or a fixed list, and a MoonBit
`Int` can represent different integer widths. Using the language's default codec would lose
these distinctions. Named definitions and recursive references must retain their source
schema identity as well.

Rust and MoonBit generated producer factories bind the appropriate item codecs to a native
stream and its writer. Use these factories for locally produced values passed through a
generated bridge, especially generated composite types and values with erased schema
distinctions. The resulting values still use the native SDK's reading, writing, and closing
APIs. TypeScript and Scala adapters likewise select codecs from the source item schema.

Receiving and forwarding an unread stream does not require creating another producer.
Pass the received stream directly to the next awaited call. The original endpoint is
transferred; item codecs run when producing or reading items, not to forward the endpoint.
After reading begins, follow the native SDK's restrictions on forwarding partially read
streams rather than expecting the bridge to buffer or recreate them.

## Ownership and failure

A stream endpoint is affine: transferring it does not create a second usable endpoint.
Generated conversion must release partial acquisitions on failure, including failures in a
later sibling field or a nested stream item. Successful output conversion hands ownership
to the caller; it must not close the returned streams as temporary values.

Clean producer completion is EOF. Producer exceptions and canonical operation cancellation
must not be converted to EOF. Bare component-model streams do not provide a recoverable
producer-supplied terminal error: represent recoverable application errors in the item type,
for example a stream of results.

Closing or dropping the readable endpoint lets a producer observe peer drop cooperatively
on a subsequent write. It does not promise to interrupt an arbitrary pending source pull or
complete remote cleanup before a later invocation. Writes provide backpressure; forwarding
must not introduce eager reads, an unbounded queue, or a transport pump.

## Verification

The shared `tests/bridge_gen/fixtures.rs` streaming schema exercises recursive items,
dynamic sibling streams, streams of streams, and schema distinctions erased by language
types. Language-specific generator tests compile real native producer/consumer usage.
Cross-component tests live under `tests/app/` because building generated consumers invokes
external language toolchains.

Keep the external streaming tests and non-streaming guest tests passing when changing shared
type naming or codec emission. Source assertions alone cannot verify endpoint ownership,
failure cleanup, cancellation, or backpressure; those require runtime tests.
