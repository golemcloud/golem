---
name: golem-tools-middleware-effect
description: Defines and calls Golem tools and attaches Effect-native typed or universal middleware. Use for generated built-in or MCP imported clients, tool providers, and middleware composition.
---

# Effect tools and middleware

## Call an existing tool through a generated client

Load `golem-generating-tool-clients` to select a built-in release or MCP import.
Add the tool to the Effect component's `dependencies.tools`; `golem build`
automatically generates its Effect client. Configure its runtime binding separately,
then wire the generated source into the consumer's TypeScript paths and includes.
No explicit bridge selection is needed for the dependency.
Call the built-in `bash` release `0.2.1` from an Effect agent method:

```typescript
import { Effect } from 'effect';
import { client as bash } from 'bash-tool-guest-client';

const program = Effect.gen(function* () {
  const result = yield* bash.run("printf 'hello\\n'", '', 600);
  return result.stdout;
});
```

Check the result's exit code when a nonzero shell exit should fail the method.
For streaming commands, start and collect the invocation inside `Effect.scoped`.
The collector preserves the result and both output outcomes independently.
Generated calls fail with `ToolRuntimeError<E>` and require the tool runtime;
they do not call the MCP server directly.

## Define a tool and attach middleware

Build a definition with `Tool.toolDefinition(name).body(...)`. A provider finishes it with `.implement({ camelCaseName: handler })`; a caller uses `Tool.client(definition)`. Handlers and clients return Effects and stream stdin/stdout/stderr with independent Effect `Stream` values. Drain both outputs concurrently when both are declared.

Use `Middleware.typed({ name, parameters: Middleware.NoParameters, presented, handler })` when the presented tool shape is known. Use the universal middleware API for schema-agnostic, shape-preserving policy. Universal middleware can be installed environment-wide or on selected tool bindings; typed (monomorphic) middleware can only be installed on tool bindings. Per-tool merge modes never remove environment-wide middleware. Forward input, output, permission cards, and streams at most once; capability handles are affine. Golem checks each underlying call against the calling agent's tool permissions, using the command and arguments the next layer receives, so an adapter that changes them needs the agent to hold grants for both forms.

The default world supports ordinary, standalone-middleware, and combined components. Standalone middleware can be attached and deployed independently; unused agent and tool discovery returns empty lists.
