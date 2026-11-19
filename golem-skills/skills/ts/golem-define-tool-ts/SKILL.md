---
name: golem-define-tool-ts
description: "Defines and implements a typed Golem tool in TypeScript. Use when creating a tool provider or command schema."
---

# Define a Golem tool in TypeScript

Build a definition with `toolDefinition(...).body(...)`, then register one implementation object:

```typescript
import { ok, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';

export const echo = toolDefinition('echo')
  .version('1.0.0')
  .body((body) =>
    body.positional('value', z.string()).returns(z.string()),
  );

echo.implement({
  echo: async ({ value }) => ok(value),
});
```

The implementation key is the generated command name. Use the body builder for positional,
option, flag, tail, stdin/stdout/stderr, result, and declared-error metadata; use Standard Schema values
such as Zod schemas for typed data. Provider handlers return `ok(result)` or a declared `err(...)`.
Call `.implement(...)` only once per definition.

## Declare HTTP routes

Use the SDK's `http` helpers on the definition and command builders:

```typescript
import { http, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod/v4';

const report = toolDefinition('report')
  .http(http.mount('/reports'))
  .command('status', (c) =>
    c.http(http.get('/status/{id}')).body((b) => b.positional('id', z.string())),
  )
  .command('admin', (c) =>
    c.http(http.prefix('/admin')).command('purge', (p) =>
      p.http(http.delete('/purge/{id}')).body((b) => b.positional('id', z.string())),
    ),
  );
```

A mount enables HTTP declarations; it does not expose undeclared commands. Every nonroot ancestor needs an endpoint or prefix declaration. Prefixes emit no route. Names and aliases never supply path segments. A parent endpoint also supplies its suffix to descendants. The example declares `GET /reports/status/{id}` and `DELETE /reports/admin/purge/{id}`.

Endpoint options include `routeId`, `headers`, `responseHeaders`, `mode`, and `durableStreams`. A query template such as `?format={format}` and `headers: { 'X-Token': 'token' }` bind declared inputs. GET, HEAD and DELETE implicitly bind remaining eligible inputs from query parameters. Path captures require scalar inputs; query and header bindings reject body-only complex values. Mount and prefix captures must name globals in scope at that node. With plain mode and stdin, every required input must bind outside the JSON body. Platform-managed response headers are rejected. Tool declarations do not accept agent auth, CORS or system path variables.

Omit `mode` to use `durableStreams` for a non-idempotent command with any declared standard stream, otherwise `plain`. Only the command's own idempotent annotation controls this default; an explicit mode wins. Omit `routeId` to use the dot-separated canonical command path. Grafting uses the parent mount, discards the child's standalone mount, and retains the child-root HTTP declaration unless the graft site overrides it. HTTP metadata does not change CLI/MCP invocation compatibility. These declarations describe publication; they do not themselves implement HTTP serving.
