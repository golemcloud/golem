---
title: "Golem 1.5 features — Part 11: Bridge libraries"
date: "2026-04-20T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-11-bridge-libraries"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part11-bridges/"
---

## Introduction

This is part 11 of a series covering Golem 1.5 features, releasing end of April 2026. This installment assumes reader familiarity with Golem. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Calling agents from outside of Golem

Multiple methods exist for invoking Golem agents externally, including HTTP exposure, MCP, CLI, REPL, and the REST API. Version 1.5 introduces "bridge libraries" — self-contained packages providing fully type-safe clients for a specific agent, to be used in non-golem applications.

The Rust implementation uses `reqwest` for HTTP requests, while TypeScript uses `fetch`. Scala and MoonBit cannot currently generate bridge libraries as target languages, though bridges can be generated to call agents written in those languages.

### Enabling the bridge generator

Bridge generation is configurable per-agent and per-language through YAML configuration:

```yaml
bridge:
  ts:
    agents:
      - CounterAgent
  rust:
    agents: "*"
```

After running `golem build`, generated bridges appear in the `golem-temp/bridge-sdk/` directory organized by language.

### Using the bridge libraries

Generated libraries follow agent-to-agent communication conventions, providing type-safe clients with static constructor methods like `get` for upserting agents and configuration options for server selection (Local, Cloud, or custom deployments).

```typescript
import { CounterAgent, configure } from "counter-agent-client/counter-agent-client.js";

configure({
  server: { type: "local" },
  application: "bridgetest",
  environment: "local",
});

const c1 = await CounterAgent.get("c1");
const value = await c1.increment();
```

```rust
use counter_agent_client::CounterAgent;
use golem_client::bridge::GolemServer;

CounterAgent::configure(
    GolemServer::Local,
    "bridgetest",
    "local"
);

let c1 = CounterAgent::get("c1").await?;
let value = c1.increment().await?;
```

### Method variants

Agent methods support multiple invocation patterns: standard awaited calls, trigger-only invocations, and scheduled execution at specified times.

<!-- The blog post does not include Scala or MoonBit bridge examples; Golem 1.5 does not ship bridge generators for Scala and MoonBit as target languages. -->
