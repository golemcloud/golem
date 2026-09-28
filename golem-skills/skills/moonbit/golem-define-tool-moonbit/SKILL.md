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
