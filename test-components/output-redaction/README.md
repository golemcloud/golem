# Output-redaction test middleware

This is a test-only fixture, not a built-in or a production middleware release. It is built and
copied by the normal test-component workflow and is not provisioned at registry startup.

`output-redaction` is a universal tool middleware that replaces configured literal byte sequences
from stdout and literal UTF-8 substrings from selected structured result fields before they reach
the caller. Stderr, declared errors, output terminals, and opaque resource values pass through
unchanged.

## Policy language

Each installation has this parameter shape:

```yaml
structured:
  - selector: $.customer.token
    pattern: sk_live_
    replacement: "[redacted]"
stdout:
  - pattern: sk_live_
    replacement: "[redacted]"
```

Patterns and replacements are literals, not regular expressions. They must be nonempty and no
longer than 4096 UTF-8 bytes. Rules are applied in declaration order and replacement bytes are not
matched again. Duplicate patterns are rejected because their precedence is usually accidental.

Structured selectors use a bounded, rooted subset of JSONPath:

- `$` selects the result root.
- `.field` selects a record field; field names contain ASCII letters, digits, `_`, or `-`.
- `[N]` selects a tuple, list, or fixed-list element by zero-based index.
- `[*]` selects every list or fixed-list element.

Selectors may pass transparently through `option<T>`. They do not traverse variants, maps,
results, unions, streams, or capabilities. A selected value must be an unconstrained `string` or
`text`. Enum targets and text targets with language, minimum-length, maximum-length, or regex
constraints are rejected before the leaf is dispatched: arbitrary substring substitution cannot
prove those constraints for every possible result. The transformed result is validated against
its original schema before it is returned. Secret, quota-token, permission-card, and stream values
are never inspected or cloned.

Stdout matching is byte-exact. The relay holds at most `longest pattern length - 1` bytes between
underlying chunks, so a partial sensitive value cannot escape at a chunk boundary. The current
chunk is processed immediately and is not counted as retained state. Clean finish and every
underlying failure terminal are relayed exactly; stderr is forwarded without inspection.

## Build and test

Build the component with the repository CLI:

```sh
golem build -P release --force-build --yes
golem exec -P release copy
```

The manifest copies `golem_output_redaction_release.wasm` into the parent `test-components`
directory. The artifact suffix does not imply a published release. Automated worker executor tests
install it directly and check structured selectors, arbitrary stream chunk boundaries, schema
constraints, and terminal forwarding.

```sh
cargo make test-component-middleware-unit-tests
```
