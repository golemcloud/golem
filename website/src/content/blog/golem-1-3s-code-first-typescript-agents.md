---
title: "Golem 1.3's Code-first TypeScript agents"
date: "2025-10-04"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Announcements"]
slug: "golem-1-3s-code-first-typescript-agents"
originalUrl: "https://golem.cloud/post/golem-1-3s-code-first-typescript-agents"
---

## Introduction

In the [previous post about Golem 1.3](/blog/golem-1-3s-new-javascript-engine) I explained that the new release comes with a new JavaScript engine. This engine fixes some important bugs and allows us to move faster and provide a better JS/TS experience - but it comes with a price. One of the tradeoffs was that building an actual WebAssembly component containing this engine requires the Rust toolchain - we are generating a Rust component that implements the component's (WIT) interface by delegating the calls to `rquickjs`.

But we don't want TypeScript developers to compile Rust components. Even more importantly, we don't want them to have to learn about the WebAssembly Component Model's own interface definition language, WIT, or any other WASM specific tooling; they should just write TypeScript code and Golem should deal with all the underlying complexity.

## History

Before showing how we solved this issue, let's talk about our previous approach. Up until today Golem embraced the component model - we've built it on it from the beginning, and I talked multiple times (on [LambdaConf](https://blog.vigoo.dev/posts/golem-and-the-wasm-component-model/) and on [Wasm I/O](https://blog.vigoo.dev/posts/golem-powered-by-wasm/)) about how we take advantage of various properties of it. Golem components were just arbitrary WebAssembly components, and Golem directly exposed the component's exported interface through its invocation and remote procedure call APIs. Although we provided more and more help in _building_ these components by making our CLI tool aware of WIT interfaces, relationship between components and so on, the idea still was that Golem should just work with any component built by any WebAssembly tooling.

We take a leap from this with Golem 1.3 and while the underlying technology is still the same, we decided to hide its complexity more from the users, and focus on one (and a few more later) supported language and do it in a way that our users are not exposed to the ever changing and evolving complexity of WebAssembly tooling. The first such supported language is **TypeScript**.

## The new way

Let's go through the technical details of how we are doing it!

We can avoid the need to generate and build Rust crates (when using TS) and avoid having to learn about WIT with a simple shift in Golem's approach to user defined components: we no longer support components with an arbitrary, user-defined WIT interface. There is one specific `WIT world` (a set of imports and exports) applied to every Golem component. This world _imports_ all the supported host APIs of Golem - its durability controls, forking, ability to update and query information about agents, etc. It also imports all the supported AI libraries [of the golem-ai project](https://github.com/golemcloud/golem-ai).

With this predefined set of imports, we can generate the Rust crate with [wasm-rquickjs](https://github.com/golemcloud/wasm-rquickjs/) once, at build time, and the resulting WASM will contain a QuickJs engine with all the bindings set up to work with Golem. This WASM is then packaged in our TypeScript SDK and published [on npmjs.com](https://www.npmjs.com/package/@golemcloud/golem-ts-sdk).

It's clear that this way we can provide support for the fixed set of features Golem provides. But we still want our users to be able to define their own interfaces that can be invoked through [Golem's invocation API](https://learn.golem.cloud/invoke), bound to HTTP routes, or called through RPC from one agent to another. How can we do this while having a static WIT interface which is even hidden from our users?

The answer is again a tradeoff - we give up some performance and composability coming from the component model to have something much more flexible and extensible.

The idea is that every Golem component implements the following interface:

```wit
package golem:agent;

interface guest {
  use common.{agent-error, agent-type, data-value};

  /// Initializes the agent of a given type with the given constructor parameters.
  /// If called a second time, it fails.
  initialize: func(agent-type: string, input: data-value) -> result<_, agent-error>;

  /// Invokes an agent. If create was not called before, it fails
  invoke: func(method-name: string, input: data-value) -> result<data-value, agent-error>;

  /// Gets the agent type. If create was not called before, it fails
  get-definition: func() -> agent-type;

  /// Gets the agent types defined by this component
  discover-agent-types: func() -> result<list<agent-type>, agent-error>;
}
```

This is quite low level and dynamic, so let's see what this means:

- Every Golem **component** can implement one or more **agent types**. The agent types are defined by the `agent-type` data type and the component can self-describe the set of agent types it implements, using the `discover-agent-types` exported function.
- Every **instance** of a Golem component (called worker in previous Golem versions) is a single instance of one of the **agent types** implemented by the component.
- The instance is initialized by the `initialize` exported function - this selects the agent type the instance belongs to, and passes **constructor parameters** (in form of a dynamic value of the `data-value` type). The initialize call is always the first call to an agent and it is automatically called by Golem itself.
- Once an agent is initialized, it can tell its own agent-type (with `get-definition`), and more importantly it can be **invoked** dynamically using the exported `invoke` function. This is dynamic in a sense that it is a single exported WIT function that takes the invoked **agent method**'s name as a string, and the parameters as an arbitrary `data-value` (just like with the constructor parameters). This is not type safe on the component level - but type safety is guaranteed by Golem on both the invocation side and the SDK side.

To have a better sense of what this interface is capable of, let's take a look at some parts of the `agent-type` and `data-value` types.

The `agent-type` is all the metadata available about an agent type, including its constructor and methods, with full type information. In addition to that it can contain additional metadata to help integration with AI systems for example.

```wit
record agent-type {
  type-name:    string,
  description:  string,
  %constructor: agent-constructor,
  methods:      list<agent-method>,
  dependencies: list<agent-dependency>,
}
```

Dependencies are not used at the moment, but it is going to allow us to statically know the dependency graph of agents. Both the constructor and the agent's methods are using the `data-schema` type to describe their input (parameters) and output (return type). The earlier mentioned `data-value` type is an instance of a type defined by `data-schema`.

In Golem 1.3, `data-schema` is still tightly coupled with the component model - it supports all the data types supported by the component model, but extends them with some concepts that are more agent specific. It's defined in the following way:

```wit
variant data-schema {
  /// List of named elements
  %tuple(list<tuple<string, element-schema>>),
  /// List of named variants that can be used 0 or more times in a multimodal `data-value`
  multimodal(list<tuple<string, element-schema>>),
}

variant element-schema {
  component-model(wit-type),
  unstructured-text(text-descriptor),
  unstructured-binary(binary-descriptor),
}
```

Without going into much detail, an input or output of an agent method can be either a tuple of elements, or multimodal. The tuple case is the traditional case - for example a method with three parameters would have a schema describing a 3-tuple. Multimodal, on the other hand, can be thought of as a list of variant values, where each element of the multimodal schema can appear any number of times. An actual example for such an interface can be a chat agent that accepts (and/or returns) content in multiple media formats, such as text, audio, or image.

An element of these tuple or multimodal schemas can be one of the WebAssembly component model types (`wit-type`), an unstructured text (possibly annotated with a language code) or unstructured binary (annotated with a MIME type) data.

## Code-first SDK

The above defined interface explains how we can define and implement multiple agent types without writing any WIT definitions, but using it directly would be very inconvenient.

In TypeScript, when using [wasm-rquickjs](https://github.com/golemcloud/wasm-rquickjs/), the implementation would require writing the following functions:

```typescript
export namespace guest {
  /**
   * Initializes the agent of a given type with the given constructor parameters.
   * If called a second time, it fails.
   * @throws AgentError
   */
  export function initialize(agentType: string, input: DataValue): Promise<void>;
  /**
   * Invokes an agent. If create was not called before, it fails
   * @throws AgentError
   */
  export function invoke(methodName: string, input: DataValue): Promise<DataValue>;
  /**
   * Gets the agent type. If create was not called before, it fails
   */
  export function getDefinition(): Promise<AgentType>;
  /**
   * Gets the agent types defined by this component
   * @throws AgentError
   */
  export function discoverAgentTypes(): Promise<AgentType[]>;
}
```

It's inconvenient to write these by hand, but it's just TypeScript code - we can just write a library on top of it that makes it more user friendly!

### A pure TypeScript approach

One possibility would be to write a TypeScript library that exports functions to define the data schemas and agent type metadata, then connect an implementation to each. This can be made very type safe using advanced type level techniques - for example defining the schema would not only assemble a `DataSchema` value, but would also track the value type (such as a tuple of the agent method parameters) on the type system level. Then when attaching an actual implementation to the defined method, the compiler can infer that the parameters are having these types.

The SDK would expose some kind of global registry to define these well typed agents in, possibly using a builder-like fluent API. In the end it just implements the above WIT exports itself using the registered agent definitions.

Parts of this would be very similar to how some TypeScript libraries define schemas for validation. It is important to mention though that with Golem validating the input is not necessary - the runtime guarantees that the agent constructor and agent methods are only called with values matching the types from the agent type metadata.

With a library like this, you could define the simplest possible stateful Golem agent, a counter, in a way like this:

```typescript
defineAgentType({
  typeName: "counter",
  description: "An example Golem agent implementing a counter",
  id: { name: type_string() },
  state: (_id) => {
    value: 0;
  },
  methods: [agentMethod("increment", {}, { result: type_u32() })],
}).implement({
  initialize: (name) => {
    console.log(`Counter ${name} created`);
  },
  increment: (state) => {
    state.value += 1;
    return state.value;
  },
});
```

Here the agent's identity (constructor parameters), state and agent methods would be defined in terms of typed versions of `DataSchema`, with schema constructors such as `type_string()`. Then in the `implement` call the object passed would require implementations for the constructor and the methods using the data schema to infer their parameter and return types.

Note that this is just a sketch - we decided to _not_ implement a library like this.

### Golem's TypeScript SDK

The approach we chose is to take advantage of **decorators** and the [ts-morph library](https://ts-morph.com/) to make writing the agents even more convenient. The primary advantage is _not_ having to specify data schemas at all. When compiling the TypeScript code the types are extracted and made available for the SDK in runtime - it can transform the TypeScript AST to the matching `DataSchema` values and `AgentType` definitions, or fail in a user friendly way if something in the user's code is not supported.

Before looking into the details, see how the same _counter_ example looks like with the actual Golem TS SDK!

```typescript
import { BaseAgent, agent, description } from "@golemcloud/golem-ts-sdk";

@agent()
class CounterAgent extends BaseAgent {
  private readonly name: string;
  private value: number = 0;

  constructor(name: string) {
    super();
    this.name = name;
  }

  @description("Increases the count by one and returns the new value")
  async increment(): Promise<number> {
    this.value += 1;
    return this.value;
  }
}
```

Every class annotated with `@agent` becomes an **exported agent type**. There are not many restrictions - they have to extend `BaseAgent`, and every type used in the constructor and the methods of this class must be something that the SDK can express with a `DataSchema`. But it is fully automatic - we can define and use complex custom data types in the agent's interface without having to manually write any schema for them.

A component can define as many agent types (classes decorated as `@agent()` as necessary). The only reason to have multiple _components_ in an application is to have different update policies or other configuration for them.

In addition to automatically converting these annotated classes into agent type definitions and their implementations, the SDK also provides support for **remote agent calls**. Every agent class gets a static method on it (put there by the decorator) called `get`. The get method has "get-or-create" semantics. Every agent is identified by their constructor parameters. There can be only one instance with a specific constructor parameter value. The get-or-create semantics of the `get` method guarantees that this is true (it creates a new agent if it did not exist yet, otherwise returns a reference to the existing one).

In the above example our counter has a string identifier called `name`. As I wrote earlier, in Golem every component instance corresponds to a single agent. This means that referring to and calling any other agent (either the same type, or another type) ends up being an "agent-to-agent" remote procedure call under the hood.

With the SDK this is very convenient - we can use the `get` method to get a remote agent reference by just passing the constructor parameters to it, then call any of the agent's methods directly on the agent reference:

```typescript
const anotherCounter = CounterAgent.get("not-my-name");
const newValue = await anotherCounter.increment();
```

What happens under the hood is that when compiling the TypeScript agent (using `golem-cli app build`), first a pre-compilation step called `golem-typegen` analyses the source code and emits a JSON describing the agent classes and their method parameter and return types. This step uses [ts-morph](https://ts-morph.com/), which is a wrapper of the TypeScript compiler API, to get the AST of the user code and extract the necessary information by traversing that.

This generated JSON gets bundled into the final JS code and it's used by the decorator logic to implement the `discoverAgentTypes`, `initialize` and `invoke` methods.

### Future

Note that this approach of having the low-level agent interface, and building SDKs on top means that even though we chose a specific approach we support as the official way of writing TypeScript agents for Golem, it is easy to experiment with alternative techniques and publish alternative SDKs. Also the official SDK can be extended with different styles of agent definitions, if we decide to do so.

It is also possible to experiment with supporting other languages. The next Golem release after 1.3 will bring back support for using Rust. With Rust we are getting the same code-first agent SDK as with TypeScript, only instead of ts-morph generated ASTs and decorators it is going to be built on proc macros and type classes.

## Composition

There are two additional interesting build steps hidden behind the scenes that we haven't talked about yet. Both are implemented using WebAssembly **component composition** - something that we no longer expose to our users (to avoid having to fall back to WASM tooling and hand-written WIT specs) but we still use it under the hood.

### Base WASM and user JS

The result of the compilation steps described above - `golem-typegen` and then compiling the TypeScript code itself - results in a single JS file. On the other hand we want to have a Golem component - which is a WASM component. I explained above that by restricting a Golem component to always implement a specific world, we can precompile the JavaScript engine with all the import and export bindings. This precompiled WASM is part of the `golem-ts-sdk` NPM package. We still have to somehow inject the compiled JavaScript file into this component!

What we do is the following:

- The precompiled WASM **imports** a specific WIT interface with a single method that returns a JS string
- We generate _another WASM component_ bundling the compiled JS and exposing a single **export** that returns this string.
- The import matches the export so we can **compose** the two WASM components into one.

Generating raw WASM component bytecode might not be very difficult for this particular use case, but we wanted something more scalable. As you will see in the next section, this is not the only place where we needed to generate WASM on the fly.

Instead we are using [MoonBit](https://www.moonbitlang.com/) to compile high level MoonBit source code directly into WASM. We can even embed the MoonBit compiler in Golem's CLI so there are no external dependencies for our users, thanks to that the [compiler itself is running on WASM](https://www.moonbitlang.com/blog/moonbit-wasm-compiler). MoonBit is a high level and very exciting new programming language, and what makes it a perfect choice for this job is that it generates really concise WASM bytecode.

I have written a small helper crate for Rust, [moonbit-component-generator](https://github.com/golemcloud/moonbit-component-generator) that embeds the compiler and helps with generating these small wrapper components.

For injecting the scripts, we basically give the WIT of the component we want to generate:

```rust
let mut component = MoonBitComponent::empty_from_wit(
  r#"
    package golem:script-source;

    world script-source {
      export get-script: func() -> string;
    }
  "#,
  Some("script-source"),
)?;
```

Then we define the MoonBit bindings based on this WIT, and add our implementation as a MoonBit source string:

```rust
component
  .define_bindgen_packages()
  .context("Defining bindgen packages")?;

let mut stub_mbt = String::new();
uwriteln!(stub_mbt, "// Generated by `moonbit-component-generator`");
uwriteln!(stub_mbt, "");
uwriteln!(stub_mbt, "pub fn get_script() -> String {{");
for line in script.lines() {
  uwriteln!(stub_mbt, "    #|{line}");
}
uwriteln!(stub_mbt, "}}");

component
  .write_world_stub(&stub_mbt)
  .context("Writing world stub")?;
```

And finally build the WASM component:

```rust
component
  .build(None, target)
  .context("Building component")?;
```

The resulting WASM is ready to be composed with the prebuilt JavaScript engine. For this we can use the [wac-graph](https://crates.io/crates/wac-graph) Rust crate (or the `wac` command line tool when doing it by hand).

### WIT wrapper for Agents

As I wrote in the _History_ section of this post, Golem built directly on the WASM component model. Its component exports and invocation mechanism directly depend on analyzing the component's WIT exports and providing ways to invoke them remotely.

We've also created a [scripting language called **Rib**](https://learn.golem.cloud/rib) that is used for defining HTTP APIs on top of Golem components as well as for playing with them through a REPL; this scripting language uses WASM-specific syntax and naming conventions to allow users to call component model interfaces through Golem. Rib here is just used as an example of something still depending on component exports. For more information check the linked official documentation, or [Afsal Thaj's presentation from Wasm I/O 2025](https://www.youtube.com/watch?v=vgrZxN0t-N0).

Although we decided to move our user experience to the higher level agent interface described in this post, many parts of Golem still depend on components exporting their interfaces on the component model level. Most of these can be evolved in future versions to directly know about the agent type metadata, etc., but this is going to be a migration process through multiple Golem releases.

Until then, without some additional trick, we would be in a very bad situation when for example using Rib to define HTTP API mappings or just manually trying to invoke an agent method. The only relevant exported WIT interface is the low-level dynamic one - each invocation would need to be put together by passing agent type name and agent method name strings, and assembling parameter values by converting them to the `data-value` component model value representation. This would be completely unusable in practice.

What we did for this release to avoid rewriting a large part of the system is that as part of the build process we generate **a static wrapper** that exports a WIT interface that represents the user-defined agent types coming from code.

The steps to do this are the following:

- First the user's code is compiled to JS and composed with the base WASM, as explained before
- Then we instantiate this WASM and call the `discoverAgentTypes` export - we get back the agent type metadata
- Using this we generate a **WIT interface per agent type**, with an `initialize` function representing the constructor and one exported function for each agent method
- We generate a **MoonBit** implementation of these interfaces. These implementations encode/decode the values into `data-value` and call the underlying component's dynamic `initialize` and `invoke` exports.
- Finally we compose this wrapper with the original component too. The resulting WASM will have all the exports and imports as the original one, but in addition to that will also export static, well typed interfaces for each agent type defined in the TypeScript code.

Even though this static wrapper will most likely not be needed in the next major Golem release, the technique may remain used if we want to use a Golem component in context of another WASM environment or tool.
