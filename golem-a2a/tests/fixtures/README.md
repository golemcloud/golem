# A2A v1.0.1 examples

`spec-v1.0.1-examples.json` preserves the JSON, HTTP-with-JSON, and SSE examples
from [the v1.0.1 specification](https://github.com/a2aproject/A2A/blob/v1.0.1/docs/specification.md).
Each entry gives its section, source line, and verbatim fenced source. Values
come from that text, not from an upstream SDK. The source is Apache-2.0 licensed.

Complete conforming examples round-trip exactly through the typed wire codec.
The `Illustration` entries explain why an example cannot satisfy that assertion:
the specification also contains fragments, placeholders, invalid JSON, legacy
examples, and examples contradicting the normative schema. In particular §8.5
uses `security` instead of `securityRequirements`, and §6.4/§6.5 show problem
bodies instead of §11.6 errors.
These are not extra accepted wire formats.

The §6.2 stream omits REQUIRED identifiers in its updates. Some message examples
also omit `messageId`. Their decoded values re-encode with those fields restored
under §5.7; the fixture or test states those defaults explicitly.
REQUIRED-field tests follow `specification/a2a.proto` at the same tag.
Empty REQUIRED arrays are tested for encoding presence, not semantic validity
(§5.7 requires at least one element in a valid message).
