---
name: golem-call-tool-ts
description: "Calls a typed Golem tool from TypeScript. Use when invoking a tool provider and handling tool or RPC failures."
---

# Call a Golem tool from TypeScript

Bind a client from the same definition, or use `toolClientDefinition` for a caller-owned subset:

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

Commands without declared stdout return a Promise of their result. Commands with required or
optional stdout return a `StartedToolInvocation`; consume its `stdout` and await its `result`, or
use `.collect()` to collect both. `ToolCallError.cause.tag` is `tool`, `rpc`, or `unknown-error`.
Protocol errors use `rpc` with `cause.error.tag === 'protocol-error'`.
