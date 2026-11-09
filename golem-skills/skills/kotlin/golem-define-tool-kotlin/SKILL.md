---
name: golem-define-tool-kotlin
description: "Defining a Golem tool in Kotlin. Use when the user asks to expose a top-level function as a CLI-style tool, handle stdin/stdout/stderr streams, or return structured tool errors."
---

# Defining a Tool (Kotlin)

A **tool** is a stateless, CLI-shaped capability exported next to a component's agents. Annotate a **top-level function** with `@Tool`:

```kotlin
import cloud.golem.annotations.Tool

@Tool(name = "greet", description = "Print a greeting for someone")
fun greet(name: String, times: Int): String = (1..times).joinToString("\n") { "hello, $name" }
```

- The tool name is the root command; each function parameter becomes a positional, in declaration order. The return value is the result. Types follow the same mapping as agent methods.
- Throw `ToolException(tag, message)` to return a structured tool error (`invalid-tool-name`, `invalid-input`, `constraint-violation`, `invalid-result`; any other tag becomes `invalid-input`). Any other exception is returned as `invalid-input` with its message.
- One package per component, shared with any `@Agent` classes (and `@ToolMiddleware` classes). Tool names must be unique.
- A non-empty command path is rejected with `invalid-command-path` (one tool = one root command, no subcommands). `custom-error` cannot be raised yet.
- `@Tool` attributes: `name`, `description`, and the stream flags `stdin`, `stdout`, `stderr` (all `false` by default); `@Command(description)` documents a parameter.

## Streams

```kotlin
@Tool(name = "kt-upper", description = "Uppercases stdin", stdin = true, stdout = true, stderr = true)
fun ktUpper(mode: String, io: ToolStreams): Int {
    var total = 0
    while (true) {
        val chunk = io.stdin?.read() ?: break              // null at the clean end of input
        total += chunk.size
        io.stdout?.write(chunk.decodeToString().uppercase().encodeToByteArray())
    }
    io.stderr?.write("done:$total".encodeToByteArray())
    return total
}
```

`ToolStreams` is not a positional; each stream is `null` if the caller did not attach it. `read()` returns the next non-empty chunk (`readAll()` collects), `write()` returns `false` once the caller stopped reading, `finish()`/`fail(ToolStreamFailure)` end the stream. Normal return finishes open writers; an exception drops them (the caller sees `abandoned`). Limit: 16 MB of stream data per invocation.

## Reflection

`ToolHost.describeAll()` / `describe(name)` return the full command tree (`ToolInfo`: commands, options, flags, positionals, annotations) of registered tools.

## Related skills

- `golem-call-tool-kotlin`
- `golem-tools-middleware-kotlin`
