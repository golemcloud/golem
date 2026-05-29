---
title: "NEW: Golem 1.3 Release"
date: "2025-10-15"
author: "John A. De Goes"
tags: ["Announcements"]
slug: "new-golem-1-3-release"
originalUrl: "https://golem.cloud/post/new-golem-1-3-release"
---

<iframe allowfullscreen="true" frameborder="0" scrolling="no" src="https://www.youtube.com/embed/91-CH1TZG3o" title="Golem 1.3 Launch Event"></iframe>

We’re a few months out from [announcing our refocus](/blog/golem-prepares-for-major-refocus-on-agentic-applications) on agentic applications. Though at the time, the announcement was purely connected to our shift in marketing, I outlined a roadmap that would take Golem into new territory focused specifically on agentic use cases.

I’m happy to announce that in our new 1.3 release, which is launching on October 15, we have made significant progress toward this goal.

In this post, I will outline the major changes you can expect in this release, and give you a peek at our short-term priorities between now and the end of the year.

## Terminology

A big update in Golem 1.3 is that we have rewritten all developer documentation, as well as some technical artifacts, and updated terminology to reflect our new agent-first focus.

The biggest change here is that Golem speaks in terms of “agents”, not “workers”. In the old terminology, an “agent” would be described as a stateful singleton resource running in a worker. Now, we just call this an agent, and ditch “workers” altogether.

Despite the fact that Golem increasingly adopts agent terminology, you can, of course, use Golem to build non-AI applications, and we have plenty of examples of this.

## Natively TypeScript

Golem has long supported the Rust programming language, and not just because Golem is written in Rust. Rust has leading-edge support for the WASM component model (internally used by Golem for state capture and security), which builds on WASM to provide a standardized way to link across guest and host, and across different languages.

Although we are big believers in the power of the Rust programming language for building high-performance, safe distributed systems (such as Golem), the language is not used by developers for building business applications.

Rust’s target is systems-level programming: that’s where we use it, and that’s where it shines.

Yet, Golem is fundamentally a runtime for building _business_ applications. Because Rust is not often used to build business applications, Golem’s support for this wonderful programming language has not translated into business use cases for Golem.

We always knew this would be a problem, but it was one of those problems that would solve itself, eventually. The Bytecode Alliance has been hard at work bringing the WASM component model to business programming languages. Despite ongoing investments from many players (including Microsoft, Google, and others), these efforts are still a ways from fruition.

Rather than wait for the ecosystem to mature, we decided to go all-in on supporting one of the most mainstream programming languages out there: TypeScript.

Not only does the Golem 1.3 release bring TypeScript to the forefront of the Golem development experience, but in focusing on this language, we have temporarily dropped support for Rust (don’t worry for all lovers of Rust: we will bring it back soon!).

Finally, we’ve brought the power of Golem to a programming language that software developers actually use to solve business problems–and to a market that is several orders of magnitude larger than the relatively niche market for Rust tools.

## Code-First Agents

Beyond TypeScript support, the biggest and most impactful change in Golem 1.3 is that we have started _completely interning_ WASM implementation details. This means that unlike in previous releases, a developer using Golem does not need to know the following topics:

- WebAssembly
- WASM Component Model
- WASM System Interface (WASI)
- wit-bindgen
- Etc.

Rather than forcing developers to rely on a clunky WASM toolchain, Golem has introduced a long-planned feature now called _code-first agents_. The idea behind _code-first agents_ is that you can define agents with public triggers using nothing more than code.

The following code defines an agent with a constructor and a single method:

```javascript
@agent()
class RequestHandler extends BaseAgent {
    // ...
    constructor(userId: string, requestId: string) {
        super();
        this.userId    = userId;
        this.requestId = requestId;

        // Begin task...
    }

    @description("Adds some more details to the request handler about the task being performed. The result is an update of the current status.")
    async addDetails(details: TaskDetails): Promise<RequestHandlerStatus> {      
        // ...
    }
}
```

Agents of these types can be created and interacted with from a Golem API, or by other agents, all without even a trace of WASM.

Agents are uniquely identified across a distributed cluster by their constructor parameters. Meaning that if a certain type of agent has no constructor parameters, then there is only a single agent of that type in the cluster (a cluster-wide singleton).

With this dramatic simplification, agent-to-agent communication is now both code-first and type-safe, as previewed in the following code snippet:

```javascript
@agent()
class WeatherAgent extends BaseAgent {
    constructor(location: string) {
        super();
    }

    @description("Gets the current weather")
    async currentWeather(): Promise<string> {
        // ...
    }
}

// ....

const weatherInLondon = WeatherAgent.get("London")

const weather = await weatherInLondon.currentWeather()
```

These changes provide a dramatic simplification in the process of building agents and engineering distributed systems with reliable and type-safe RPC.

## Manifest & CLI

Golem CLI and the manifest file format that it uses have undergone a number of upgrades:

- Component environment variables can now reference real environment variables, providing a way to configure agents in an environment-specific way.
- New pretty JSON and YAML formats are supported, which help to integrate Golem CLI into other tools and workflows.
- A new --reset flag has been added, which can also be set in the manifest profile, which forces a reset of a local Golem server for a better development experience.
- The REPL accepts scripts as a string or from files.

Together, these improvements provide a smoother command-line experience than ever before.

## Console

Golem Cloud Console, which is the interface to our preview managed offering, has been updated to focus the interface on agentic development.

In addition, Console has gained a new time-traveling debugger, which allows exploration of interactions of an agent, as well as basic abilities to reset and recover failed agents.

## Hardening

Owing to the large number of changes in this release, our testing period extended many weeks and involved many developers, and allowed us to identify and fix a number of issues, making Golem 1.3 our most polished and hardened release yet.

## Toward the Future

Golem’s next minor release (1.4), and possibly the last in the 1.x line, will focus on continued investment in an agent-first feature set.

Among the improvements planned for this release:

- Atomic deployments, which provide a pleasing and rollback-friendly developer experience for updating infrastructure based on the declarative manifest file
- Code-first endpoints, which eliminate the need to write scripting “glue” when introducing custom APIs for agent creation and interaction
- MCP export, which allows all Golem agents to be used by other agents, including Claude Desktop, ChatGPT, and others
- Quotas, with overage strategies, such as auto-suspension and auto-resumption
- Type-safe configuration that can be customized differently for different types of agents, overridden on a per-agent basis, etc.
- Improved TypeScript SDK, and improved support for [Node.js](https://nodejs.org)
- Re-addition of support for Rust, with stretch goals of MoonBit and Scala.

These features will take Golem all the way into agent-native territory, with capabilities that will make Golem increasingly attractive for agentic application development.

## Learning More

The Golem 1.3 launch event ran live on October 15, 2025 — overview, live coding demo, and a walkthrough of key features of the Golem CLI and Console. The full recording is embedded at the top of this post.

To follow Golem Cloud across other channels:

- [Golem Cloud LinkedIn](https://www.linkedin.com/company/97878182)
- [Golem Cloud X](https://x.com/GolemCloud)
- [Ziverge on YouTube](https://www.youtube.com/@Ziverge)
