---
name: golem-tools-middleware-ts
description: "Defines typed or universal Golem tool middleware in TypeScript. Use for validation, policy, auditing, or tool adapters."
---

# Tool middleware in TypeScript

Call `.middleware(...)` on the presented definition. Omit `wraps` for transparent middleware;
provide another definition in `wraps` for an adapter:

```typescript
import { ToolInvokeError, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';

const echo = toolDefinition('echo').body((body) =>
  body.positional('value', z.string()).returns(z.string()),
);

echo.middleware({
  name: 'echo-policy',
  implementation: {
    echo: async ({ value }, { underlying }) => {
      if (value.length === 0) {
        throw new ToolInvokeError({
          tag: 'constraint-violation',
          val: 'value is empty',
        });
      }
      return underlying.echo({ value });
    },
  },
});
```

Use `universalToolMiddleware(...)` only when middleware must inspect arbitrary tool metadata and
raw typed values. Prefer typed middleware whenever the surface is known. The exact handler context
and result shape follows the command's arguments, declared errors, and streams; let TypeScript
infer it rather than casting. Use `underlying` only during the middleware handler; it may make
multiple calls before the handler settles. Do not retain it for later invocations. Do not reuse
transferred streams or permission-card handles. Returned stdout may be consumed lazily after the
handler returns.
