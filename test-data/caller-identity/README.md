# Caller identity vectors

Golden vectors for the caller model in `golem-common/src/base_model/caller/`. Every
implementation that prints, parses or compares pairs, or that builds or checks identity proofs,
checks itself against these files instead of against another implementation.

## `text-form-v1.json`: the text form of a pair

A pair is `("<issuer>","<subject>")`, with no spaces. Each string is escaped as RFC 8785
(section 3.2.2.2) serializes JSON strings:

- `\"` and `\\`;
- `\b`, `\t`, `\n`, `\f` and `\r`;
- every other control character (U+0000 to U+001F) as `\u00xx`, in lowercase hex;
- every other character as it is, including `/`, U+007F and all non-ASCII text.

A parser accepts only that spelling, so two texts are equal exactly when their pairs are equal.
It refuses, for example, `\/`, `\u0062`, uppercase hex, a raw control character, spaces and an
unquoted `*`.
The refused spellings include the escape of any character from U+0020 up (such as `\u0026`,
`\u0100` or `\u2028`) and every escaped surrogate, lone or paired.

- `roundTrip`: each `issuer` and `subject` prints as exactly `text`, and `text` parses back to
  them.
- `reject`: each `text` fails to parse.

Pairs compare byte for byte: no case folding, and no trailing-slash or Unicode normalization.
Some strings hold characters that editors may not show (U+007F, U+2028, U+2029); edit the file
with a tool that keeps them.

## `subjects-v1.json`: Golem subjects

The subjects of agents, accounts and tools under a Golem issuer. Each one starts with its kind,
and every ID is a lowercase UUID with hyphens:

- `agent:<agent-id>/<fingerprint>`: the agent's ID, never its key, and the fingerprint of this
  life of the agent, so an agent created again under the same ID is a new pair;
- `account:<account-id>`: never the account's path;
- `tool:<environment-id>/<tool-name>`: nothing of one declaration, so a tool declared again
  under the same name keeps its pair.

- `roundTrip`: the fields print as exactly `subject`, and `subject` parses back to them.
- `reject`: each `subject` fails to parse.

## `identity-proofs-v1.json`: identity proofs

An identity proof is a JWT in JWS compact form that states an agent's or a tool's own pair for
one audience. Each case gives the `input` of a proof, and the `header` and `claims` that the
input gives:

- the header is `alg: ES256`, the issuer key's `kid` and `typ: JWT`;
- `iss` and `sub` are the principal's own pair, `aud` is one audience, `exp` is 300 seconds after
  `iat`, and `jti` is unique for each proof;
- the `golem` claim holds the `kind` (`agent` or `tool`), the `environment` ID, the `account` ID
  (never the account's path), and the `agentType` of an agent or the `tool` name of a tool.

A proof for AWS STS role assumption (`awsRoleAssumption: true`) also repeats the values of the
`golem` claim as AWS session tags in the claim `https://aws.amazon.com/tags`, under
`principal_tags`: `golem:environment`, `golem:account`, `golem:kind`, and `golem:agent-type` or
`golem:tool`. Each tag value is a one-element list, as in the example of the AWS documentation,
and there are no transitive tag keys. No other proof carries this claim.

The AWS page "Passing session tags in AWS STS" is not consistent on the form of a tag value: its
example shows a one-element list, while its text says "Each principal tag is a single string
value" and "You must pass a single value for each session tag". The real-AWS test of the 1.5
plan decides between the list and the plain string; the vectors follow the example until then.

The builder refuses to make a proof for AWS role assumption when a tag value breaks the rules
that the AWS pages state: at most 256 characters; only the characters of the pattern
`[\p{L}\p{Z}\p{N}_.:/=+\-@]*` that the AWS STS API reference gives for `Tag` values
(https://docs.aws.amazon.com/STS/latest/APIReference/API_Tag.html; Unicode letters, separators and
numbers by general category); no `aws:` prefix. A proof for another receiver has no such limit. No vector
holds a refused value, so the vectors do not change; the tests cover the refusals.

The vectors fix the header and the claim set, not a signature.

## `conversion-from-1.6-v1.json`: 1.6 principals

A 1.6 principal reaches 2.0 only inside the bridge export: in an agent's SDK snapshot envelope,
and as the agent's recorded init principal. The bridge and each 2.0 SDK convert it to a reference,
or to no reference, as these vectors say. No shared code implements the conversion.

The vectors start from the tag/val form below. The 1.6 server holds its own principals in
another form, for example `{"type":"GolemUser","accountId":...}` and `{"type":"Anonymous"}`, so
the 7.6 bridge translates that form to the tag/val form first, as the Go SDK does, and then
converts.

Each case gives:

- `principal`: the 1.6 principal in the JSON form that the Rust, TypeScript, Effect, MoonBit and
  Scala SDKs write into their 1.6 snapshot envelopes: `{"tag": ..., "val": ...}`, with camelCase
  field names, every UUID as a lowercase string with hyphens, and the OIDC `claims` as a JSON
  string: what 1.6 writes with `serde_json::to_string` of the ID token claims. The Go SDK
  writes another form, so it translates its own form first. An implementation reads these
  strings as they are: a UUID in another spelling (upper case, braces, `urn:uuid:`, no
  hyphens) is not an input.
- `loginMapping`: the external login of the principal's account, or `null`. 1.6 holds only
  GitHub logins, written `{"provider": "github", "externalId": ..., "login": ...}`, with the
  numeric GitHub user ID as canonical decimal text (no sign, no leading zero) and the login
  name. The other shape, `{"provider": "oidc", "issuer": ..., "subject": ...}`, stands for the
  person pairs that 7.6 "People" maps to another provider. No other provider is defined. Which
  mapping wins when an account has several is the decision of the 7.6 plan.
- `importedAgent`: for an `agent` principal, `{"agentId": ..., "fingerprint": ...}`, or `null`
  when the agent did not carry over. `agentId` is the 2.0 agent ID that the bridge's ID mapping
  gives; `fingerprint` is the fingerprint of the agent that the principal names. Both are
  lowercase UUIDs with hyphens.
- `expected`: the `kind` of the principal it refers to and the `pair` (in the text form above)
  of the reference, or `null` for no reference.

`golemIssuer` is the installation's Golem issuer. The conversion:

| 1.6 principal | Reference |
|---|---|
| `oidc` | kind `user`, the token's `issuer` and `sub` exactly as written, never the email |
| `golem-user` with an external login mapping | kind `user`, the person pair of the mapping: for GitHub `("https://github.com","<numeric GitHub user ID>")`, never the login name |
| `golem-user` without a mapping | kind `account`, `("<Golem issuer>","account:<account-id>")` |
| `agent` | kind `agent`, `("<Golem issuer>","agent:<2.0 agent ID>/<fingerprint>")` |
| `agent` that did not carry over | no reference |
| `anonymous` | no reference |

## Consumers

- Rust (the source of truth): `pair/tests.rs`, `subject/tests.rs`, `identity_proof/tests.rs` and
  `conversion_from_1_6_tests.rs` in `golem-common/src/base_model/caller/`.

When the format changes, change these files in the same commit; the Rust tests fail until both
agree.
