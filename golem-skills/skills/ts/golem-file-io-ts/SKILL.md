---
name: golem-file-io-ts
description: "Reading and writing files from a TypeScript Golem agent. Use when the user asks to read files, write files, or do filesystem operations from agent code in TypeScript."
---

# File I/O in TypeScript Golem Agents

## Overview

Golem TypeScript agents run in a QuickJS-based WASM runtime that provides `node:fs` and `node:fs/promises` modules for filesystem operations. Use these to read files provisioned via the Initial File System (IFS) or to write temporary data.

To provision files into an agent's filesystem, load the `golem-add-initial-files` skill. To understand the full runtime environment, load the `golem-js-runtime` skill.

## Reading Files

### Synchronous

```typescript
import * as fs from 'node:fs';

const content = fs.readFileSync('/data/config.json', 'utf-8');
```

### Asynchronous

```typescript
import * as fsp from 'node:fs/promises';

const content = await fsp.readFile('/data/config.json', 'utf-8');
```

### Reading Binary Files

```typescript
import * as fs from 'node:fs';

const buffer = fs.readFileSync('/data/image.png');
// buffer is a Buffer (Uint8Array subclass)
```

## Writing Files

Only files provisioned with `read-write` permission (or files in non-provisioned paths) can be written to.

```typescript
import * as fs from 'node:fs';

fs.writeFileSync('/tmp/output.txt', 'Hello, world!');
```

### Asynchronous

```typescript
import * as fsp from 'node:fs/promises';

await fsp.writeFile('/tmp/output.json', JSON.stringify(data));
```

## Checking File Existence

```typescript
import * as fs from 'node:fs';

if (fs.existsSync('/data/config.json')) {
  const content = fs.readFileSync('/data/config.json', 'utf-8');
}
```

## Listing Directories

```typescript
import * as fs from 'node:fs';

const files = fs.readdirSync('/data');
// files is string[]
```

## Complete Agent Example

```typescript
import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';
import * as fs from 'node:fs';

@agent()
class FileReaderAgent extends BaseAgent {
  constructor(readonly name: string) {
    super();
  }

  async readGreeting(): Promise<string> {
    const content = fs.readFileSync('/data/greeting.txt', 'utf-8');
    return content.trim();
  }

  async writeLog(message: string): Promise<void> {
    fs.appendFileSync('/tmp/agent.log', message + '\n');
  }
}
```

## Key Constraints

- Use `node:fs` or `node:fs/promises` — they are fully supported in the QuickJS runtime
- Files provisioned via `golem-add-initial-files` with `read-only` permission cannot be written to
- The filesystem is per-agent-instance — each agent has its own isolated filesystem
- File changes within an agent are persistent across invocations (durable state)
