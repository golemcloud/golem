---
title: "Golem 1.5 features — Part 14: OpenTelemetry"
date: "2026-04-22T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-14-opentelemetry"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part14-otlp/"
---

## Introduction

This blog post is part of a series showcasing new features in Golem 1.5, releasing at the end of April 2026. I assume readers have familiarity with Golem and direct them to previous posts for background information. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Oplog Processor Plugins

Oplog processor plugins, previously unfinished in earlier Golem versions, are now mature enough for real use in version 1.5. These are special agents implementing a specific interface that receive batches of operation log entries for processing.

Oplog processors guarantee exactly once delivery of entries to processors and attempt to maintain instances on every executor node for in-process delivery without network overhead. They function as full Golem agents with durable execution capabilities.

```rust
struct Example;

impl OplogProcessorGuest for Example {
    fn process(
        account_info: AccountInfo,
        config: Vec<(String, String)>,
        component_id: ComponentId,
        agent_id: AgentId,
        metadata: AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), String> {
        // Process the batch of oplog entries here
        Ok(())
    }
}

export_oplog_processor!(Example
    with_types_in golem_rust::oplog_processor);
```

### Plugin Installation

Plugins are registered via YAML metadata files using the CLI, then referenced in application manifests to enable them for specific components. Configuration can be customized through key-value parameters per installation.

```yaml
name: my-oplog-processor
version: v1
description: My custom oplog processor
icon: icon.svg
homepage: https://example.com
specs:
  type: OplogProcessor
  component: "/path/to/my-oplog-processor.wasm"
```

```bash
golem plugin register my-plugin.yaml
```

```yaml
components:
  my:component:
    plugins:
      - name: my-oplog-processor
        version: v1
        parameters:
          batch_size: "100"
          target_endpoint: "https://logs.example.com"
```

## OpenTelemetry Plugin

Golem 1.5 introduces a built-in OpenTelemetry plugin called `golem-otlp-exporter` that exports agent telemetry data through OTLP/HTTP, supporting traces, logs, and metrics.

```yaml
components:
  otlp-demo:ts-main:
    templates: ts
    plugins:
      - name: golem-otlp-exporter
        version: 1.5.0
        parameters:
          endpoint: "http://localhost:4318"
          signals: "traces,logs,metrics"
```

The plugin configuration includes endpoint URLs, signal selection, custom headers, and service naming modes based on agent identity or type.

### Features

**Traces** capture spans for built-in operations and custom user-defined spans, with proper trace ID propagation through HTTP requests.

```typescript
import { startSpan, currentContext } from "golem:api/context@1.5.0";

const span = startSpan("my-operation");
span.setAttribute("env", { tag: "string", val: "production" });
span.setAttributes([
  { key: "service", value: { tag: "string", val: "my-service" } },
  { key: "version", value: { tag: "string", val: "1.0" } },
]);

// ... do work ...

const ctx = currentContext();
console.log(`trace_id: ${ctx.traceId()}`);

span.finish();
```

```typescript
import { tracingChannel } from "node:diagnostics_channel";

// Now any traceSync/tracePromise call automatically creates a Golem span
// with attributes from the context object's properties
const dc = tracingChannel("my-operation");

const result = dc.traceSync(
  () => {
    // ... do work ...
    return 42;
  },
  { method: "GET", url: "/api/data", env: "production" } // these become span attributes
);
```

```rust
use golem_rust::bindings::golem::api::context;

// Start a span and set attributes
let span = context::start_span("my-operation");
span.set_attribute("env", &context::AttributeValue::String("production".to_string()));
span.set_attributes(&[
    context::Attribute { key: "service".to_string(), value: context::AttributeValue::String("my-service".to_string()) },
    context::Attribute { key: "version".to_string(), value: context::AttributeValue::String("1.0".to_string()) },
]);

// ... do work ...

// Read back context
let ctx = context::current_context();
println!("trace_id: {}", ctx.trace_id());

span.finish();
```

```scala
import golem.host.ContextApi

val span = ContextApi.startSpan("my-operation")
span.setAttribute("env", ContextApi.AttributeValue.StringValue("production"))
span.setAttributes(List(
  ContextApi.Attribute("service", ContextApi.AttributeValue.StringValue("my-service")),
  ContextApi.Attribute("version", ContextApi.AttributeValue.StringValue("1.0")),
))

// ... do work ...

val ctx = ContextApi.currentContext()
println(s"trace_id: ${ctx.traceId()}")

span.finish()
```

```moonbit
// Using with_span for automatic lifecycle management:
@context.with_span(
  "my-operation",
  attributes=[("env", "production"), ("service", "my-service"), ("version", "1.0")],
  fn(span) {
    // ... do work ...
    // Add more attributes dynamically if needed:
    span.set_attribute("step", "processing")

    let ctx = @context.current_context()
    println("trace_id: " + ctx.trace_id())
  },
)
```

**Logs** forward all console and logging output to OTLP collectors.

```typescript
console.log("Hello from TypeScript!");
console.debug("This is a debug log entry");
```

```rust
use log::debug;

println!("Hello from Rust!");
debug!("This is a debug log entry");
```

```scala
println("Hello from Scala!")
Logging.log(LogLevel.Debug, "", "This is a debug log entry")
```

```moonbit
println("Hello from MoonBit!")
@log.debug("This is a debug log entry")
```

**Metrics** provide 24+ counters and gauges tracking invocations, memory, resources, errors, transactions, and snapshots, annotated with service and agent identification.
