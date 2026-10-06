# Snapshot multipart profile

SDK-owned envelopes use `multipart/mixed; boundary=<B>`. Emit CRLF framing;
decoders also accept LF framing. Fix the framing newline E from the first
delimiter, then remove exactly one E before each subsequent delimiter. Header
lines may independently use CRLF or LF; bare CR and folded headers are invalid.

Delimiter lines are exactly `--B` followed by E, or `--B--` followed by E or EOF.
Require a closing delimiter and reject epilogues, transport padding, and suffix
text in structural positions. Accept no prefix or one leading E. Boundary
prefix near misses and mid-line boundary text remain opaque payload bytes.
Collision detection includes body-start delimiter lines and the body's appended
framing E, which can complete a trailing delimiter candidate.

Boundaries are case-sensitive, 1–70 ASCII characters from
`A-Za-z0-9'()+_,./:=?-`, quoted when required by MIME syntax. Reject duplicate
boundary parameters and malformed quoting. SDK parts have exactly one
Content-Type and one `Content-Disposition: attachment; name="..."`; header keys
are case-insensitive. Reject duplicate names, duplicate structural headers,
unsupported headers, and header injection.

Exactly one physical `state` part has Content-Type `application/json` and UTF-8
JSON `{ "version": 1, "principal": ..., "state": ... }`. It need not arrive
first. SDKs own metadata and principal serialization; reject duplicate metadata
keys. Binary-only state is JSON null. User parts are `part:<logical-name>`;
managed SQLite images are `db:<name>`. Logical user names match
`[A-Za-z0-9_][A-Za-z0-9_.-]*` and are case-sensitive. `part:state` and
`part:__proto__` are valid. Bare ASCII MIME type/subtype values normalize to
lowercase; user MIME parameters are rejected. User bodies remain opaque even
when marked application/json. Unknown namespaces are invalid. Simple snapshot
modes must not silently discard user parts; unsupported DB composition fails.

`framing.json` contains shared payload cases with independently specified hex
expectations. Generic host rendering consumes the framing cases but does not
enforce SDK envelope, namespace, or principal policy. SDK envelope tests belong
at each SDK's envelope boundary, separately from these generic framing cases.
