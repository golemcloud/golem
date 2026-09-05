# Stream Session Public Protocol v1 vectors

These files are normative cross-language vectors for
`docs/reference/stream-session-public-protocol-v1.md`.

Every generated streaming runtime must consume the files directly. JSON held
in a `canonical` or `input` string is intentionally not parsed by the fixture
file itself; this preserves duplicate fields, invalid syntax, and exact output
bytes. Binary envelope bytes use padded standard base64 in the fixture file.

The token vectors use a public test-only HMAC key. It must never be configured
outside tests.
