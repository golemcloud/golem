---
name: golem-call-from-external-ts
description: "Calling Golem agents from external TypeScript/Node.js applications using generated bridge SDKs. Use when the user wants to invoke agents from outside the Golem platform, from a Node.js server, script, or any TypeScript application."
---

# Calling Agents from External TypeScript Applications

## Overview

Golem can generate typed TypeScript client libraries (bridge SDKs) for calling agents from any external Node.js or TypeScript application — a web server, a CLI tool, a serverless function, etc. The generated client communicates with the Golem server's REST API and provides a fully typed interface matching the agent's methods.

## Step 1: Enable Bridge Generation

Add a `bridge` section to `golem.yaml`:

```yaml
bridge:
  ts:
    agents: "*"                    # Generate for all agents
    # Or list specific agents:
    # agents:
    #   - MyAgent
    #   - my-app:billing
    outputDir: ./bridge-sdk/ts     # Optional custom output directory
```

The `agents` field accepts `"*"` (all agents), or a list of agent type names or component names (`namespace:name`).

## Step 2: Generate the Bridge SDK

Run:

```shell
golem build
```

Bridge generation happens automatically as part of the build. Alternatively, generate bridges without a full build:

```shell
golem generate-bridge
golem generate-bridge --language ts
golem generate-bridge --agent-type-name MyAgent
```

This produces an npm package per agent type (e.g., `my-agent-client/`) in the configured output directory (or `golem-temp/bridge-sdk/ts/` by default).

## Step 3: Install and Build the Generated Package

In the generated package directory:

```shell
cd bridge-sdk/ts/my-agent-client
npm install
npm run build
```

Then add it as a dependency in your external TypeScript project:

```json
{
  "dependencies": {
    "my-agent-client": "file:../path/to/bridge-sdk/ts/my-agent-client"
  }
}
```

## Step 4: Use the Generated Client

```typescript
import { MyAgent, globalConfig } from 'my-agent-client';

// Configure the Golem server connection
globalConfig({
  server: { type: 'local' },
  application: 'my-app',
  environment: 'local',
});

// Get or create an agent instance
const agent = await MyAgent.get('my-instance');

// Call methods — fully typed parameters and return values
const result = await agent.doSomething('input');
console.log('Result:', result);
```

## Server Configuration

The `server` field supports three modes:

```typescript
// Local development server (http://localhost:9881)
{ type: 'local' }

// Golem Cloud
{ type: 'cloud', token: 'your-api-token' }

// Custom deployment
{ type: 'custom', url: 'https://my-golem.example.com', token: 'your-token' }
```

## Phantom Agents

To create multiple agent instances with the same constructor parameters, use phantom agents:

```typescript
import { v4 as uuidv4 } from 'uuid';

const agent = await MyAgent.getPhantom(uuidv4(), 'shared-name');
```

Or generate a random phantom ID automatically:

```typescript
const agent = await MyAgent.newPhantom('shared-name');
```

## Agent Configuration

If the agent has local configuration fields, use the `WithConfig` variants:

```typescript
const agent = await MyAgent.getWithConfig(
  'my-instance',
  someConfigValue,    // config parameter (optional)
);
```

## Generated Package Dependencies

The generated npm package depends on `@golemcloud/golem-ts-bridge` (the shared bridge runtime) and `uuid`. It is built with TypeScript and outputs ES module format with type declarations.

## Key Points

- Bridge generation runs during `golem build` — agents must be built first so their type information is available
- The generated code is fully typed — method parameters and return types match the agent definition
- All custom types (records, variants, enums, flags) are generated as corresponding TypeScript types
- The client uses `fetch` for HTTP communication
- Each agent type gets its own npm package with `package.json`, `tsconfig.json`, and a `.ts` source file
- Run `npm install && npm run build` in the generated package before using it
