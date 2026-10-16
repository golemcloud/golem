---
name: golem-call-tool-ts
description: "Calls a typed Golem tool from TypeScript. Use when invoking generated built-in or MCP imported tool clients, a tool provider, or handling tool and RPC failures."
---

# Call a Golem tool from TypeScript

## Call an existing tool through a generated client

Load `golem-generating-tool-clients` to select a built-in release or MCP import,
add it to the consumer's `dependencies.tools`, and configure its runtime binding.
`golem build` automatically generates the TypeScript client without an explicit
bridge selection. Wire the generated package into the consumer. Do not copy the
existing tool's schema into `toolDefinition`.

For the built-in `bash` release `0.2.1`, call from an agent method:

```typescript
import { BashClient } from 'bash-tool-guest-client';

const result = await BashClient.newClient().run("printf 'hello\\n'", '', 600);
if (result.exitCode !== 0) throw new Error(result.stderr);
return result.stdout;
```

CLI-generated methods take positional arguments, not the object used by
definition-owned clients below. Non-streaming methods return a Promise of the
result; streaming methods return a `StartedToolInvocation` synchronously. Use
`.collect()` and check its result, stdout, and stderr settlements separately.
Generated clients report declared errors with `tag: 'tool'` and RPC failures with
`tag: 'rpc'`; they do not wrap them in the definition-owned `ToolCallError` class.

## Call from a definition owned by the application

When the application owns the definition, bind a client from it, or use
`toolClientDefinition` for a caller-owned subset:

```typescript
import { ToolCallError, toolClientDefinition, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';

const echo = toolDefinition('echo').body((body) =>
  body.positional('value', z.string()).returns(z.string()),
);
const client = toolClientDefinition(echo).client('echo');

try {
  const value = await client.echo({ value: 'hello' });
  console.log(value);
} catch (error) {
  if (error instanceof ToolCallError && error.cause.tag === 'tool') {
    console.error(error.cause.error);
  } else {
    throw error;
  }
}
```

Commands without declared stdout or stderr return a Promise of their result. Commands with either
output return a `StartedToolInvocation`; consume its independent `stdout` and `stderr` streams and
await its `result`, or use `.collect()` to drain every declared output concurrently. Stderr bytes do
not imply failure. `ToolCallError.cause.tag` is `tool`, `rpc`, or `unknown-error`.
Protocol errors use `rpc` with `cause.error.tag === 'protocol-error'`.
