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
    external:
      agents: "*"                    # Generate for all agents
      # Or list specific agents:
      # agents:
      #   - MyAgent
      #   - my-app:billing
      outputDir: ./bridge-sdk/ts     # Optional custom output directory
```

The `agents` field accepts `"*"` (all agents), or a list of agent type names or component names (`namespace:name`).

## Step 2: Generate the Bridge SDK

The recommended approach is to declare the bridge in `golem.yaml` (as shown above) and let `golem build` produce the SDK as part of the normal build:

```shell
golem build --yes
```

This produces an npm package per agent type (e.g., `my-agent-client/`) in the configured output directory (or `golem-temp/bridge-sdk/ts/` by default). Re-running `golem build` after agent changes keeps the generated client in sync automatically.

Avoid invoking `golem generate-bridge` manually — it exists as a low-level escape hatch, but the manifest-driven flow above is the supported way to keep bridges configured, reproducible, and up to date.

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

// Address the keyed agent of these constructor parameters; the server creates it on the
// first invocation when it is missing
const agent = MyAgent.upsert('my-instance');

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

### Reconnect window of streaming calls

A streaming call reconnects with a growing delay (50 ms up to 3 s) while the server accepts neither
its start nor a resume; HTTP 429 and 5xx on the upgrade and retryable rejections are retried too.
`reconnectWindowMs` in the configuration bounds the time without progress: the default is
`DEFAULT_RECONNECT_WINDOW_MS` (5 minutes), and `'unlimited'` keeps trying until the call's
`AbortSignal` aborts.

```typescript
configure({ server: { type: 'local' }, application: 'my-app', environment: 'local', reconnectWindowMs: 30_000 });
```

Past the window the call rejects with `StreamingUnreachableError` (from
`@golemcloud/golem-ts-bridge`), with `endpoint`, `reconnectWindowMs` and `lastCause`.

## Creating Agents and Addressing Them by ID

A durable agent type also has `create` and `getById`. `create` creates a new agent with a new
agent ID (a UUID) and no key, even when other agents have the same constructor parameters:

```typescript
const agent = await MyAgent.create('shared-name');
const agentId = await agent.agentId(); // save it to reach this agent again

const same = await MyAgent.getById(agentId);
```

`getById` never creates an agent: an unknown ID, or the ID of an agent of another type, rejects
with a `GolemServiceError` of status 404.

## Agent Configuration and Environment

Pass local configuration values and environment variables in the options of `create`. With
`keyed: true` it creates the keyed agent of the constructor parameters instead, and rejects with
a `GolemServiceError` of status 409 when that agent exists:

```typescript
const agent = await MyAgent.create('my-instance', {
  keyed: true,
  config: { someConfigField: someConfigValue },
  env: { LOG_LEVEL: 'debug' },
});
```

`upsert` takes no configuration or environment.

## Generated Package Dependencies

The generated npm package depends on `@golemcloud/golem-ts-bridge` (the shared bridge runtime) and `uuid`. It is built with TypeScript and outputs ES module format with type declarations.

## Key Points

- Bridge generation runs during `golem build` — agents must be built first so their type information is available
- The generated code is fully typed — method parameters and return types match the agent definition
- All custom types (records, variants, enums, flags) are generated as corresponding TypeScript types
- The client uses `fetch` for HTTP communication
- Each agent type gets its own npm package with `package.json`, `tsconfig.json`, and a `.ts` source file
- Run `npm install && npm run build` in the generated package before using it
