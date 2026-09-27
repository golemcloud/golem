# Golem SDK for MoonBit — Examples

Example agents and tool middleware built with MoonBit Golem SDK 0.5.x, targeting **Golem 1.6**.

## Agents

- **Counter** — Simple stateful counter with increment, decrement, and get-value methods
- **TaskManager** — Task management with custom data types (`Priority` enum, `TaskInfo` struct)
- **VisionAgent** — Multimodal agent accepting text or image input
- **RpcExampleAgent** — Demonstrates agent-to-agent RPC using generated client stubs

## Agent tools

`golem_moonbit_examples/canonical_tools.mbt` contains the canonical `grep` and `git` agent-tool
definitions shared with the Rust and Scala SDK parity suites. It demonstrates root commands, pure
dispatchers, nested subtrees, inherited globals, positionals/options/flags, refinements,
constraints, result formatters, typed custom errors, stdin/stdout injection, and full `UInt64`
bounds.

The `agents` build step generates:

- `golem_tools.mbt` — descriptors, registration, custom error schemas, and dispatchers.
- `golem_tool_clients.mbt` — typed clients for root commands and nested subcommand trees.

Both files are generated and must not be edited manually. To regenerate them directly while
iterating on this repository:

```sh
cd ../golem_sdk_tools
moon run cmd -- agents ../golem_sdk_example1 \
  --component-dir golem_moonbit_examples
```

## Tool middleware

`golem_tool_middleware_examples/middleware.mbt` is a separate **pure middleware component**. It
contains:

- `MessagePolicy`, a monomorphic policy that forwards allowed messages once and short-circuits
  blocked messages without calling the underlying tool;
- `audit`, a universal pass-through that inspects runtime metadata and forwards opaque carriers;
- `FileAdapter`, a compact monomorphic adapter that presents `PublicFiles` while expecting
  `Storage` from the next inner layer.

The default local template runs both generators, embeds SDK world `agent-guest`, and produces the
middleware component. The same world supports ordinary, standalone-middleware, and combined
components; unused agent and tool discovery returns empty lists.

Generated `golem_reexports.mbt`, `golem_tool_middlewares.mbt`, and the generated
entries in `moon.pkg` must not be edited manually. To regenerate this component directly:

```sh
cd ../golem_sdk_tools
moon run cmd -- reexports ../golem_sdk \
  ../golem_sdk_example1/golem_tool_middleware_examples
moon run cmd -- agents ../golem_sdk_example1 \
  --component-dir golem_tool_middleware_examples
```

See the SDK and generator READMEs for the exact monomorphic/universal signatures, capability and
stream lifetime rules, same-package tool-shape limitation, and runtime ownership of placement and
chain ordering.

## HTTP routers and live files

`golem_moonbit_examples/http_router.mbt` defines three independently named types:

* `Site`: `#derive.http_router` with `#derive.mount("/site")`. Repeated
  `#derive.static_file("/assets/*", "/preferred/$1")` declarations preserve order;
  only a missing file falls through to the next mapping, then the handler.
* `StaticSite`: immutable files without a synthetic handler. Routers may also
  register only a provider, both roles, or neither role.
* `FileOwner`: an ordinary durable `#derive.agent` with
  `#derive.expose_files("/value", "/value.txt")`. Its mount binds the complete
  constructor identity. GET initializes the owner and reads its current file;
  the typed PUT endpoint updates that same owner's file.

`#derive.http_handler` and `#derive.openapi_provider` designate ordinary public
instance methods with arbitrary names. A router has at most one of each, no
other exported methods, no identity arguments, no snapshots, and ephemeral mode.
An injected `@config.Config[T]` constructor argument is allowed and is not part of
its wire identity. Use ordinary generated agent clients for dependencies.
Routers themselves have no generated ordinary RPC client.

Import `golemcloud/golem_sdk/http` for `HttpRequest`, `HttpResponse`, and
`HttpHeader`. Bodies are `@schema.AgentStream[Array[Byte]]`, not `Bytes`
(which uses the binary schema). Request query `None` differs from `Some("")`.
Headers preserve duplicate entries and raw bytes. Use `UInt16` for response
status. The example forwards one chunk at a time, stops on `PeerDropped`, and
uses `on_unstarted_drop` to release the request when a HEAD/bodyless response
disposes its unstarted producer. Stream EOF or disposal is not proof that the
whole invocation completed; active host cancellation may terminate Wasm.

The OpenAPI provider returns `String`. `@http.openapi_json(Json)` is an optional
deterministic serializer: sorted object keys, preserved array/opaque example
content, valid Unicode/finite numbers, at most 1 MiB UTF-8 and 64 container levels.
The host owns OpenAPI 3.1 validation, reference checks, namespacing, and merging;
neither the helper nor provider fetches external references.

After a local deployment, exercise the examples against the local gateway:

```sh
curl http://localhost:9006/site/assets/message.txt
curl http://localhost:9006/static/message.txt
printf 'incremental echo' | curl --data-binary @- http://localhost:9006/site/echo
curl http://localhost:9006/files/alice/value
curl -X PUT -H 'content-type: application/json' \
  --data '{"value":"updated"}' http://localhost:9006/files/alice/value
curl http://localhost:9006/openapi.json
```

File sources are absolute public paths with an optional terminal `/*`; subtree
targets end in `/$1`. Public segments are percent-decoded once, filesystem targets
are not URI-decoded, and duplicate compiled source/target pairs are rejected.
The same source with different targets is valid. Provision immutable files with
read-only manifest entries. Live-file owners must be durable, non-phantom, and
bind each constructor identity argument exactly once in their mount.

## Building

Requires [golem-cli](https://github.com/golemcloud/golem/releases), `wasm-tools`, and the MoonBit toolchain.

```sh
# Build both the ordinary and pure middleware components for local (debug):
golem build -L

# Build both components for release:
golem build -L -P release
```

## Deploying

```sh
# Deploy to a local Golem server:
golem deploy -L -Y

# Deploy with reset (clears existing agent state):
golem deploy -L --reset -Y
```
