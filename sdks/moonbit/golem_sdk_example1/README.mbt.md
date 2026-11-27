# Golem SDK for MoonBit — Examples

Example agents and tool middleware built with MoonBit Golem SDK 0.5.x, targeting **Golem 1.6**.

## Agents

- **Counter** — Simple stateful counter with increment, decrement, and get-value methods
- **TaskManager** — Task management with custom data types (`Priority` enum, `TaskInfo` struct)
- **VisionAgent** — Multimodal agent accepting text or image input
- **RpcExampleAgent** — Demonstrates agent-to-agent RPC using generated client stubs

## Static configuration

`config_agent.mbt` derives `AppConfig` and its nested `DatabaseConfig`. The generated
constructor loads `@config.Config[AppConfig]` once. The agent keeps it and reads
`config.value`; `ConfiguredAgent.settings` returns the captured application name
and database address.

Use a field annotation such as `#derive.config(default="5432")` for a numeric
default or `#derive.config(default="\"localhost\"")` for a string default. The
generator emits the literal canonical schema JSON as declaration metadata. Golem
fills missing static settings from it below deployment and creation values. A
supplied invalid value still fails. Reading config never applies an SDK fallback.

Config fields, including fields inside static data records, use lowerCamelCase:
`app_name` becomes `appName`. Ordinary method schemas keep their native names.
`#derive.config(name="exactName")` sets an explicit config path segment. Catalog
paths inside JSON selectors are data and remain exact. Named data records use
`#derive.golem_schema`; tuples retain optional positions and recursive records
retain their shared schema definitions. Existing custom schema traits can describe
discriminated unions without changing their branch tags or discriminators.

An omitted default differs from `default="null"` on an optional field. Defaults
must be literal JSON strings, not functions or MoonBit default expressions. A
nested config group declares defaults on its leaves; a data record can have a
record default. Secret fields cannot declare static defaults. Shared bindings,
capabilities and special-field construction are not static defaults.

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
  only a missing file falls through to the next mapping, then the handler. Repeated
  `#derive.file_response_header("name", "value")` declarations add ordered response
  headers to files served by the mount.
* `StaticSite`: immutable files without a synthetic handler. Routers may also
  register only a provider, both roles, or neither role.
* `FileOwner`: an ordinary durable `#derive.agent` with
  `#derive.expose_files("/value", "/value.txt")`. Its mount binds the complete
  constructor identity. GET reads an existing owner's current file, and an
  owner that was never created answers 404; the typed PUT endpoint updates
  that same owner's file. The same repeated
  `#derive.file_response_header("name", "value")` annotation configures live file
  responses. Omitting it produces an empty header list.

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
