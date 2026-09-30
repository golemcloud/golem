---
name: golem-permission-card-ts
description: "Transfers opaque permission cards through TypeScript agent and tool schemas. Use when delegated authority crosses a call boundary."
---

# Permission cards in TypeScript

Declare the capability with `s.permissionCard({ polymorphic })`:

```typescript
import { s, toolDefinition } from '@golemcloud/golem-ts-sdk';

const delegate = toolDefinition('delegate').body((body) =>
  body
    .positional('card', s.permissionCard({ polymorphic: false }))
    .returns(s.permissionCard({ polymorphic: false })),
);
```

The runtime value is an opaque raw permission-card resource received from a host or another call.
The SDK does not expose a public constructor for forging one. Successful encoding transfers
ownership before the call completes. Never reuse a transferred handle, even if the invocation
subsequently fails. Do not inspect, stringify, persist, or duplicate permission-card values. Set
`polymorphic: true` only when the card schema may contain owner or resource-id slots.
