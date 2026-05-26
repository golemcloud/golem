---
title: "Golem 1.5 features — Part 15: MoonBit"
date: "2026-04-23T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-15-moonbit"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part15-moonbit/"
---

## Introduction

This post is part of a series showcasing new features of Golem 1.5. MoonBit is an interesting new language with many nice features, notably the capability to compile to very small WASM binaries (and very quickly). Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## MoonBit Support

MoonBit becomes a first-class supported language in Golem 1.5. Unlike other supported languages, it generates compact WebAssembly, enabling faster agent instantiation. While bridge generators and a dedicated REPL remain unavailable, the TypeScript REPL is fully usable with MoonBit agents.

## Code Examples

A counter agent uses struct annotations with `#derive.agent`. Public methods become agent methods.

```moonbit
///|
/// Counter agent in MoonBit
#derive.agent
struct Counter {
  name : String
  mut value : UInt64
}

///|
/// Creates a new counter with the given name
fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}

///|
/// Increments the counter and returns the new value
pub fn Counter::increment(self : Self) -> UInt64 {
  self.value += 1
  self.value
}

///|
/// Returns the current value of the counter
pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}
```

Custom types require `#derive.golem_schema` annotation:

```moonbit
#derive.golem_schema
struct MyData {
  field1: String
  field2: UInt
}

#derive.golem_schema
enum Status {
  Active
  Inactive(String)
}
```

Agent-to-agent communication uses generated client types:

```moonbit
CounterClient::scoped("my-counter", fn(counter) raise @common.AgentError {
  counter.increment()
  counter.increment()
  let value = counter.get_value()
  value
})
```

The framework supports HTTP endpoints through `#derive.mount` and `#derive.endpoint` annotations:

```moonbit
#derive.agent
#derive.mount("/moonbit-counters/{name}")
struct Counter {
 // ...
}

#derive.endpoint(post="/increment")
pub fn Counter::increment(self : Self) -> UInt64 {
  // ..
}
```

## Implementation Details

The MoonBit SDK comprises a code transformation tool (written in MoonBit) and a library. The tool parses source code, identifies derive attributes, and generates necessary implementations. The build workflow is hidden in a golem-managed build template.

Every other feature mentioned in this series is available for MoonBit, with documentation and skill catalogs supporting the language.
