# Web fetch

A Golem WASM component exporting the read-only `web-fetch` tool. It retrieves bounded textual
content from HTTP and HTTPS URLs and follows redirects under a strict safety policy. HTML is
returned as decoded source by default and can optionally be converted to readable text.

The URL is the only required argument. Each invocation may also set the public options
`--timeout-ms` (default 10,000, range 1–30,000), `--max-response-bytes` (default 2,097,152, range
1–5,242,880), `--max-redirects` (default 5, range 0–10), and `--convert-html-to-text` (default
false). Their Rust parameter names use underscores. Supplied safety values cannot exceed the
component's compiled ceilings. Setting the redirect limit to zero disables following redirects,
so a response that requires another request returns a redirect-limit error.

Completed fetch results and typed errors are durable and replay without another HTTP request. If
execution is interrupted before the result is recorded, recovery may retry the GET request.

Build and copy all production built-in tool artifacts from the repository root:

```sh
cargo make build-builtin-tools
```

Regenerate the third-party license report after dependency changes:

```sh
cargo about generate about.hbs --target wasm32-wasip2 --locked \
  --output-file licenses/THIRD_PARTY_LICENSES.html
```
