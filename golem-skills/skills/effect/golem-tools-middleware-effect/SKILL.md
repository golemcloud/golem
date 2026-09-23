---
name: golem-tools-middleware-effect
description: Defines and calls Golem tools and attaches Effect-native typed or universal middleware. Use for tool providers, clients, and middleware composition.
---

# Effect tools and middleware

Build a definition with `Tool.toolDefinition(name).body(...)`. A provider finishes it with `.implement({ camelCaseName: handler })`; a caller uses `Tool.client(definition)`. Handlers and clients return Effects and stream stdin/stdout with Effect `Stream`.

Use `Middleware.typed({ name, presented, handler })` when the presented tool shape is known. Use the universal middleware API only when every tool must be intercepted. Forward input, output, permission cards, and streams exactly once to `underlying`; capability handles are affine.

The default world supports ordinary, standalone-middleware, and combined components. Standalone middleware can be attached and deployed independently; unused agent and tool discovery returns empty lists.
