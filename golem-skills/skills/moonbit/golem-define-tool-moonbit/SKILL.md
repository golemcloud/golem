---
name: golem-define-tool-moonbit
description: "Defines a typed Golem tool provider in MoonBit. Use when creating tool commands, metadata, arguments, or errors."
---

# Define a Golem tool in MoonBit

Annotate an empty struct with `#derive.tool` and implement commands as public static methods:

```moonbit
/// Echo text.
#derive.tool("echo", version="1.0.0")
struct Echo {}

/// Echo one value.
#derive.arg("value", scope="positional")
pub fn Echo::echo(value : String) -> String {
  value
}
```

Use `#derive.command`, `#derive.arg`, `#derive.constraint`, `#derive.result`, and `#derive.error`
for explicit command metadata. Custom schema values need `#derive.golem_schema`. `golem build`
generates registration and typed clients; never edit `golem_tool_clients.mbt` or other generated
files.

## Declare HTTP routes

```moonbit
#derive.tool("report")
#derive.mount("/reports")
struct Report {}

#derive.endpoint(get="/status/{id}")
pub fn Report::status(id : String) -> String {
  id
}
```

This declares `GET /reports/status/{id}`. Add `#derive.endpoint(prefix="/admin")` to a `#derive.command(subtree="Administration")` method to supply a prefix without emitting a route. Every nonroot ancestor needs an endpoint or prefix. Names and aliases do not supply path segments; parent endpoints supply their suffix to children too. Grafting uses the parent mount, discards the child's standalone mount, and retains the child-root declaration unless the graft site overrides it.

Use `mode="plain"` and `route_id="status"` on the endpoint, `#derive.endpoint_header("X-Token", "token")`, and `#derive.endpoint_response_header("X-Result", "ready")` for additional metadata. Query templates such as `?format={format}` bind declared inputs. Durable Streams slots use `#derive.durable_stream_slot(input="stdin", name="upload", content_type="text/plain")` or `output="stdout"`/`"stderr"`. Load limits require **quoted numeric attributes**, for example `#derive.durable_streams(max_concurrent_readers_per_stream="7", max_append_requests_per_second_per_stream="11")`.

Path captures require scalar inputs; query/header sources reject body-only complex values. GET, HEAD and DELETE implicitly bind remaining eligible inputs from query parameters. Mount/prefix captures must name globals in scope. Plain stdin routes cannot leave required inputs in the JSON body. Tools reject agent auth/CORS/system variables and platform-managed response headers.

Omitted mode resolves to `durableStreams` for a non-idempotent command with any declared standard stream, otherwise `plain`. Only the command's own idempotent annotation controls the default; explicit mode wins. Default route IDs are dot-separated canonical command paths. HTTP declarations do not change invocation compatibility or implement serving.
