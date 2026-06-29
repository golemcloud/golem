---
name: golem-call-from-external-moonbit
description: "Calling Golem agents from external MoonBit applications using generated bridge SDKs. Use when the user wants to invoke agents from outside the Golem platform, from a standalone MoonBit CLI or native application."
---

# Calling Agents from External MoonBit Applications

## Overview

Golem can generate typed MoonBit client libraries (bridge SDKs) for calling agents from any external MoonBit application — a CLI tool, a background job, or any native MoonBit program. The generated client communicates with the Golem server's REST API and provides a fully typed interface matching the agent's methods.

The bridge target language is independent of the agent's source language, so this works for agents written in any language, including MoonBit.

## Step 1: Enable Bridge Generation

Add a `bridge` section to `golem.yaml`:

```yaml
bridge:
  moonbit:
    agents: "*"                       # Generate for all agents
    # Or list specific agents:
    # agents:
    #   - MyAgent
    #   - my-app:billing
    outputDir: ./bridge-sdk/moonbit   # Optional custom output directory
```

The `agents` field accepts `"*"` (all agents), or a list of agent type names or component names (`namespace:name`).

## Step 2: Generate the Bridge SDK

The recommended approach is to declare the bridge in `golem.yaml` (as shown above) and let `golem build` produce the SDK as part of the normal build:

```shell
golem build --yes
```

This produces a MoonBit module per agent type (e.g., `my-agent-client/`) in the configured output directory (or `golem-temp/bridge-sdk/moonbit/` by default). Re-running `golem build` after agent changes keeps the generated client in sync automatically.

Avoid invoking `golem generate-bridge` manually — it exists as a low-level escape hatch, but the manifest-driven flow above is the supported way to keep bridges configured, reproducible, and up to date.

## Step 3: Add the Generated Module as a Local Dependency

The generated `my-agent-client/` directory is a self-contained `moon` module. Its `moon.mod.json` declares an agent-derived module name matching the generated directory name (for example, `my-agent-client`), so multiple generated bridge modules can be used from the same external project. The module bundles its own runtime, only depends on `moonbitlang/async` and the MoonBit core library, and builds for the `native` target.

Add it to your external MoonBit project as a local path dependency in `moon.mod.json`. Also depend on `moonbitlang/async` directly — your own `async fn main` entry point needs it imported (see the next step), and the version must match the one the generated module pins (`0.19.2`):

```json
{
  "name": "my-org/external-client",
  "preferred-target": "native",
  "deps": {
    "my-agent-client": {
      "path": "../golem-temp/bridge-sdk/moonbit/my-agent-client"
    },
    "moonbitlang/async": "0.19.2"
  }
}
```

The `path` points to the directory containing the generated module's `moon.mod.json`. Since the client uses the native HTTP transport, `preferred-target` is set to `native` for your standalone app.

Then run `moon install`.

## Step 4: Use the Generated Client

Import the generated `client` package, the bundled `runtime` package, and `moonbitlang/async` in your `moon.pkg`. The `moonbitlang/async` import is required by MoonBit to use an `async fn main` entry point, even if you never reference `@async` directly:

```moonbit
import {
  "moonbitlang/async" @async,
  "my-agent-client/client" @client,
  "my-agent-client/runtime" @runtime,
}
```

Then call the agent. All generated constructors and methods are `async` and run on the `moonbitlang/async` event loop:

```moonbit
async fn main {
  // Configure the Golem server connection
  @client.MyAgent::configure(@runtime.Local, "my-app", "local")

  // Get or create an agent instance
  let agent = @client.MyAgent::get("my-instance")

  // Call methods — fully typed parameters and return values
  let result = agent.do_something("input")
  println(result)
}
```

Build and run the native binary:

```shell
moon build --target native
moon run --target native src/main
```

## Server Configuration

`@runtime.GolemServer` has three variants:

```moonbit
// Local development server (http://localhost:9881)
@runtime.Local

// Golem Cloud
@runtime.Cloud("your-api-token")

// Custom deployment
@runtime.Custom("https://my-golem.example.com", "your-token")
```

`configure(server, appName, envName)` sets the connection shared by every generated client in the module.

## Phantom Agents

To create multiple agent instances that share the same constructor parameters, use phantom agents. The phantom id is the last argument, after the constructor parameters:

```moonbit
let agent = @client.MyAgent::get_phantom("my-instance", "shared-phantom-id")
```

Or generate a random phantom id automatically:

```moonbit
let agent = @client.MyAgent::new_phantom("my-instance")
```

## Triggering and Scheduling

In addition to the awaiting call, each method has fire-and-forget and scheduled variants:

```moonbit
// Fire-and-forget: enqueue the invocation without waiting for a result
agent.trigger_do_something("input")

// Schedule the invocation for a future time: the RFC 3339 timestamp comes
// first, followed by the method parameters
agent.schedule_do_something("2026-01-01T00:00:00Z", "input")
```

## Overriding Configuration

If the agent declares locally overridable configuration, the generated client
also exposes `*_with_config` constructor variants. Each declared config value
becomes an optional parameter (named `config_<path>`) appended after the
constructor parameters; pass `Some(..)` to override a value or `None` to leave it
at its default:

```moonbit
// Durable: override config when getting/creating the agent
let agent = @client.MyAgent::get_with_config("my-instance", Some("override-value"))

// With an explicit phantom id (the phantom id comes before the config overrides)
let p = @client.MyAgent::get_phantom_with_config("my-instance", "phantom-id", Some("v"))

// With a fresh random phantom id
let np = @client.MyAgent::new_phantom_with_config("my-instance", None)
```

These variants are only generated when the agent declares overridable
(non-secret) configuration.

## Multimodal Methods

When a method takes or returns multimodal input (a sequence of mixed-modality
items, such as text and images), the parameter or return type is an
`Array[Multimodal<N>]`, where `Multimodal<N>` is a generated enum with one case
per modality:

```moonbit
let reply = agent.analyze([
  Multimodal0::Text("describe this"),
  Multimodal0::Image(@runtime.BinaryUrl("https://example.com/cat.png")),
])
```

## Unstructured Text and Binary

Rich `text` and `binary` parameters and return values map to the ergonomic
`@runtime.UnstructuredText` and `@runtime.UnstructuredBinary` wrappers, each of
which is either inline content or a URL reference:

```moonbit
// Inline text with an optional BCP-47 language code, or a URL
@runtime.TextInline("hello", Some("en"))
@runtime.TextUrl("https://example.com/note.txt")

// Inline bytes with an optional MIME type, or a URL
@runtime.BinaryInline(bytes, Some("image/png"))
@runtime.BinaryUrl("https://example.com/cat.png")
```

When the agent restricts the allowed language codes or MIME types, the generated
client validates returned values against the allowed set and raises a
`@runtime.BridgeError` on a disallowed code.

## Generated Module Layout

Each agent type gets its own `moon` module directory containing:

- `moon.mod.json` — module metadata, depending on `moonbitlang/async`
- `runtime/` — the bundled runtime package (`<agent-client>/runtime`): HTTP transport, the schema-native value model, codecs, and configuration
- `client/client.mbt` — the generated, fully typed client package (`<agent-client>/client`): the agent handle, its constructors and method wrappers, and the generated MoonBit types and codecs for all custom parameter and return types

## Key Points

- Bridge generation runs during `golem build` — agents must be built first so their type information is available
- The generated code is fully typed — method parameters and return types map to MoonBit types, and all custom types (records, variants, enums, flags, unions, multimodal, unstructured text/binary) are generated as corresponding MoonBit types
- The client targets `native` and uses `moonbitlang/async` for HTTP communication; all constructors and methods are `async`
- The generated module is self-contained: it bundles its runtime and only depends on `moonbitlang/async` and the MoonBit core library
- Add the generated module as a local path dependency and run `moon install` before using it
