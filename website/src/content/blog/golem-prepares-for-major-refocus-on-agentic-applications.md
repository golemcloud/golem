---
title: "Golem Prepares for Major Refocus on Agentic Applications"
date: "2025-05-14"
# date sourced from site-deploy timestamp "Wed May 14 2025" embedded in first wayback snapshot of post (web.archive.org/web/20250521164900/https://www.golem.cloud/post/golem-prepares-for-major-refocus-on-agentic-applications)
author: "John A. De Goes"
tags: ["Industry Articles", "Product Updates"]
slug: "golem-prepares-for-major-refocus-on-agentic-applications"
originalUrl: "https://golem.cloud/post/golem-prepares-for-major-refocus-on-agentic-applications"
---

For the past two years, the Golem engineering team has been dedicated to developing a unique infrastructure centered on durable computing. This infrastructure, driven by Golem's custom execution engine, ensures software components run with automatic fault-tolerance and observability.

While durable computing technology is broadly applicable across various high-reliability domains—from financial e-commerce to complex microservice orchestration and long-running workflows—we have recognized the need for strategic focus.

This post outlines Golem's pivot towards a specialized domain: agentic applications. Despite the crowded landscape of durable computing solutions, with established players and emerging startups, Golem's distinct WebAssembly-based approach positions us uniquely to address the sophisticated demands of agentic services.

This refined focus will allow us to fully leverage Golem's strengths, compensate for current limitations, and establish Golem as the go-to solution for reliable, scalable agentic deployments.

## The Crowded Landscape of Durable Computing

The space is currently very crowded, dominated by first-mover Temporal, and joined by recent startups including Golem Cloud, DBOS.dev, Restate.dev, Trigger.dev, Inngest.com, Reboot.dev, Flawless.dev, and Hatchet.run — to say nothing of dated offerings from large players like Amazon (Step Functions), Microsoft (Durable Functions), and Cloudflare (Durable Objects & Workflows).

Golem has taken a fundamentally different approach than other players in the durable computing space, with its pros and cons. Competing offerings provide language-specific frameworks, which are used to build bespoke applications. These applications run alongside durable execution runtimes, which provide persistence of state, supervision of running processes, and recovery from failure.

Golem instead has built a custom execution engine for WebAssembly (WASM). During execution of WASM components, Golem automatically captures the state and interactions of running processes, which allows it to provide supervision and recovery without requiring bespoke applications written to a custom SDK.

## The WebAssembly Advantage and Current Limitations

Because Golem's technology is built on WASM, it comes with many benefits, such as low-latency startup, secure execution with full sandboxing, efficient use of compute resources, and high scalability. Yet, with backend WASM standards such as the Component Model and WASI still in development, most languages cannot target WASM yet.

Moreover, for the languages that do have some degree of support for WASM, including C/C++, Rust, Python, Go, Kotlin, and MoonBit, typically the process of targeting WASM requires novel and unfamiliar toolchains from the WASM ecosystem, and many existing open source libraries and frameworks cannot yet run on WASM because of dependencies that aren't yet available in WASM form.

Over time, as standards mature and more vendors deliver on commitments to support backend WASM (including Microsoft for .NET and Oracle for JVM), many of these rough edges will disappear, and the amount of software that can target both traditional architectures such as ARM, x86, and Apple Silicon, as well as WASM, will greatly expand.

## Pausing the "Lift and Shift" Vision

The maturation of WASM standards and expanded vendor support remains a future prospect. Until these developments occur, Golem's original vision for "lift and shift" — i.e. the ability to take any software project, push a button to target WASM, and then deploy the resulting component on Golem for automatic durable execution — cannot be fully realized.

If the current limitations of the WASM ecosystem prevent Golem from bringing pervasive durable computing to every language and every software project, then Golem must determine which specific use cases to prioritize. By going deep on particular use cases, and focusing on one or two programming languages, Golem can compensate for WASM immaturity through specialization.

## Strategic Market Positioning

Focusing Golem on particular use cases will close the gap between Golem's superb (if low-level) durable computing technology and particular well-defined business problems. This specialization also provides a path to sharpening marketing, documentation, and evangelism.

In surveying the market for the past 4 months, it is apparent that Temporal has done an excellent job targeting applications in finance and banking, which makes sense considering their technology is the most mature; and due to their approach to require bespoke applications written for language-specific frameworks, they already have robust support for Java and C#, the languages that dominate these industries.

Beyond finance and banking, a wide range of potential applications exists for Golem to consider. Many of these opportunities remain unsuitable for the platform, either due to the pre-release state of the backend WASM ecosystem, or because of specific features that Golem has yet to implement.

Despite these challenges, one particular application stands out as an exceptional match for both Golem's unique technological approach and the current state of WASM development.

## Finding the Perfect Fit: Agentic Services

After evaluating numerous possibilities, the Golem team has identified the perfect use case to focus on: agentic services.

Agentic services is a broad category that includes the following types of services:

- **AI Agents**. Agents operate autonomously to perform actions on behalf of or under the direction of humans. There are many different types of agents, such as coding agents, as well as general purpose agents. Though there are no-code agents, their performance is generally too unreliable for integration into business processes. Therefore, high-performing agents are often written using high-level languages and driven with both custom business logic and scoped model execution
- **AI Workflows**. Like agents, AI workflows extensively leverage AI machinery, including RAG and model execution. However, unlike agents, the number and type of model executions in an AI workflow is determined solely by prewritten code, and is at no point under the control of a model. With sufficient engineering and proper architecture, AI workflows can yield reliable results for both generative and automation use cases.

It turns out that agentic services are easy to do poorly, and extremely difficult to do correctly, and a significant factor is the quality of agentic orchestration.

## Defining Agentic Orchestration

Much has been written about models, embeddings, vector databases, indexes, tools, and various protocols such as MCP, A2A, ACP. While all of these are important to building agentic services, they are insufficient by themselves.

Agentic services require an _agentic orchestration solution_, which is responsible for orchestrating and supervising the execution of business logic, models, and tools, as well as whatever internal and third-party APIs, databases, and microservices might be used by agentic services.

Examples of orchestration solutions include LangGraph, AutoGen (by Microsoft), and CrewAI. These solutions and others like them are designed to solve a variety of pain points common to the development of both agents and AI workflows.

## Pain Points Addressed by Agentic Orchestrators

The possible pains addressed by agentic orchestration solutions include:

1. **State Management and Persistence**. Agentic services need to reliably preserve and restore their internal state across steps, user sessions, and system failures.
2. **Control Flow and Orchestration**. Implementing complex business logic requires flexible, dynamic execution flows, including support for branching, looping, and conditional logic.
3. **Integration with Tools and APIs**. Real-world applications demand seamless integration with external systems, APIs, and tools to unlock practical utility.
4. **Observability and Monitoring**. Developers require real-time visibility into service execution to validate behavior, monitor health, and quickly diagnose issues.
5. **Debugging and Testing Tools**. Efficient development hinges on robust tools for inspecting, tracing, and replaying service execution to facilitate troubleshooting.
6. **Multi-Agent Coordination**. Certain scenarios demand multiple agentic components to collaborate, share information, and effectively delegate tasks.
7. **Resilience and Error Handling**. Reliable services must detect and gracefully recover from failures, ensuring robust execution even over extended durations.
8. **Complex Process Automation**. Applications frequently need structured automation spanning multiple stages, diverse tools, and integrated components.
9. **Long-Term Memory Integration**. Services must effectively persist and leverage relevant historical knowledge beyond ephemeral context windows and sessions.
10. **Human-in-the-Loop Oversight**. Many workflows require structured pauses for human approvals, input, or guided decision-making to ensure correctness and compliance.
11. **Decision Transparency**. It is crucial for users and developers to clearly understand the rationale behind decisions made by agentic services.
12. **Deployment and Scalability**. Achieving operational scale necessitates robust infrastructure support to manage hosting environments, workloads, and dynamic resource allocation.
13. **System Interoperability**. Agentic services often must coordinate actions across diverse systems, bridging isolated tools and technologies.
14. **Security and Compliance**. Handling sensitive data and critical operations securely demands comprehensive authentication, auditing capabilities, and policy enforcement.
15. **Cost and Token Optimization**. Controlling operational costs effectively requires optimized usage of compute resources and API calls, an area frequently underserved by existing solutions.
16. **Versioning and Change Management**. Managing the evolution of services without introducing regressions depends on robust version control and mechanisms for workflow migration.

Not every orchestration solution addresses every one of these pain points. As fate would have it, however, Golem’s durable computing platform ticks many of these boxes already, often in a way that has notable advantages over pure library and framework solutions.

## Golem for Agentic Orchestration

Before discussing how Golem provides a compelling solution to agentic orchestration, it is first necessary to discuss several Golem-specific concepts:

- **Components**. Applications are built from components, which are literally components of a software application that have been compiled to or packed into WASM components. Components are portable across different architectures and can be linked to each other, enabling cross-language interoperability (such as a Python component calling a Rust component, or vice versa).
- **Workers**. Upon receiving external triggers or API invocations, Golem instantiates components into isolated, sandboxed running instances known as _workers_. These workers can handle individual events or execute continuously to manage ongoing requests. In agentic contexts, a worker typically embodies a single agent (complete with its dedicated memory and other resources), a running workflow, or a specific tool invocation. Workers are generally _durable_ rather than ephemeral.
- **Oplog**. To underpin critical functionalities like observability, fault-tolerance, recoverability, time-traveling debugging, historical analysis, and other advanced features such as snapshots, forking, and undo, Golem employs an _operation log_, or _oplog_ for short. The oplog is a complete log of all external I/O events of a worker, very similar to a commit log in a database. The oplog is generated in real-time as a worker executes, and replicated across nodes in a cluster, which powers transparent recovery after failure events.
- **Durability**. Thanks to the oplog (and, optionally, state snapshots), Golem can recreate the state of any worker at any point in time. With this ability, and the core Golem services (which include supervision of running workers), Golem is capable of providing rollbacks, fault-tolerance to any type of infrastructure failure, automatic suspension (to eliminate CPU and memory usage when workers are idle), observability, time-traveling debugging, transactional execution of code, exactly-once interaction semantics, and persistence of in-memory state and execution process.

These features provide a compelling solution to a wide range of challenges building agentic services, including the following:

1. **State Management and Persistence**. With Golem, no explicit state persistence is required, because all state, including whatever is stored in memory (context, past decisions, etc.) is automatically durable and every bit as reliable as a database.
2. **Control Flow and Orchestration**. Because of Golem’s reliability guarantees, control flow and orchestration is as simple as writing or generating ordinary code. No special SDK or separate visual tooling is required.
3. **Integration with Tools and APIs**. Golem can host not only agents and AI workflows, but also tools, and doing so enables exactly-once interaction semantics regardless of concurrent updates, failures, and cluster-level rebalancing.
4. **Observability and Monitoring**. Due to the comprehensive nature of the oplog, it is possible to examine the prior history of every worker, and have near real-time insight into activity across millions of active workers.
5. **Debugging and Testing Tools**. Golem does not have a debugger yet, but much of the backend work has been completed for a time-travelling debugger, which will let you rollback history and interact with agents and workflows at any prior point in time.
6. **Multi-Agent Coordination**. Golem’s so-called worker-to-worker communication allows efficient and reliable multi-agent coordination, and with exactly-once interaction semantics, no work is lost or duplicated.
7. **Resilience and Error Handling**. Because Golem is the execution fabric for all workers, Golem knows when failures occur and automatically applies retries for automatic resilience. When nodes in a cluster go down, Golem’s supervision detects these failures and transparently resumes worker execution on new nodes for automatic robustness.
8. **Complex Process Automation**. Golem workers can execute for milliseconds, minutes, hours, days, weeks, or even years, and even be upgraded live when newer models or logic is released, allowing the reliable automation of highly complex processes that span many tools, APIs, and human interaction points.
9. **Long-Term Memory Integration**. Although Golem does not replace the need for vector databases, indexes, and graph databases, there is no need for persistence between steps of a workflow or agent, because all in-memory state is automatically and indefinitely durable.
10. **Human-in-the-Loop Oversight**. With durable promises, workers may suspend waiting for human interaction for indefinite periods of time, or have fallback behavior when humans fail to respond in a timely fashion.
11. **Decision Transparency**. As the oplog contains a full history of all I/O, any decision that an agentic service makes can be fully understood and audited after-the-fact.
12. **Deployment and Scalability**. Golem is designed to concurrently run millions or tens of millions of workers without issue, efficiently suspending idle workers to reduce infrastructure costs.
13. **Security and Compliance**. Though Golem’s authentication and authorization capabilities are currently lightweight (pending further development), Golem has full isolation and sandboxing for every worker. It’s not possible for one worker to access the resources of another worker, or cause another worker to fail.
14. **Versioning and Change Management**. Managing the evolution of services without introducing regressions depends on robust version control and mechanisms for workflow migration.

Golem does not yet currently address all pain points in building agentic services–but its core feature set is undeniably a superpower in the quest to rapidly build highly reliable agentic services.

## An Agentic Golem Roadmap

Golem is already an exceptionally great fit for deploying and running agentic workloads–despite the fact that it was designed with more general-purpose scenarios in mind.

Thanks to its unique approach to durable computing, Golem non-invasively provides reliability (including fault-tolerance, resilience to external errors, and exactly-once interactions), security, and observability, together with durable state, transactional execution, and introspectable (and even reversible) execution history.

At the same time, by directing some engineering resources directly to agentic use cases, Golem can become the go-to solution for demanding agentic services.

Over the long road to Golem 2.0, we will be investing significant resources in the following areas, each of which enable or accelerate agentic use cases:

- **Durable Streaming**. With robust streaming for large binary payloads, and first-class support for Rust and C/C++, Golem will be well-positioned for generative audio and video use cases, including pluggable support services like transcoding.
- **Standardized APIs**. Golem’s recently released LLM interface is vendor-neutral and cross-language, with durable implementations for OpenAI, Grok, Anthropic, and Ollama. Continuing in this direction, Golem will standardize more APIs for the WASM ecosystem, spanning embeddings, vector databases, indexes, generation and transformation of audio, images, and video, and sandboxed code execution.
- **Tool Usage & Implementation**. Golem will gain the ability to export any component as an MCP server, allowing easy development of new (durable) tools, and will also gain the ability to seamlessly integrate existing MCP tools. As A2A or ACP gain traction, Golem will integrate support for these protocols so developers can stay focused on logic.
- **Visual Debugging & Analysis**. Golem will introduce high-level tooling for deeply understanding the outcomes of workflows and the interactions of agents, as well as first-class support for reverting and fixing issues discovered during analysis.
- **High-level SDK**. Golem will package-up in high-level ways core Golem capabilities like snapshotting at a known good point and reverting to previous snapshots (after a wrong turn), and traversing and analyzing history of workflows and agent for model fine-tuning and enhanced retrieval augmentation.
- **Capability-based security model**. Golem already fully isolates and sandboxes all agents, workers, and tools, but as agents and workers call out to other agentic services, or tools themselves, a compositional and transparent fine-grained security model is necessary to minimize manual plumbing of authorization information while still ensuring that delegation is safe, secure, and fully auditable.
- **Examples and documentation**. Golem has a library of templates and a full documentation center, but over time these resources will be greatly expanded and heavily focused on the agentic use cases that Golem is targeting.

Collectively, these improvements will make Golem a compelling choice for safely, reliably, and quickly bringing the power of AI into organizations and products everywhere.

## New Golem Release Available

Today we’re announcing the release of Golem 1.2.2, coinciding with LambdaConf 2025, where we hosted a highly successful hackathon around Golem. This release continues our commitment to refining the developer experience, ensuring Golem remains approachable and powerful, especially for building sophisticated agentic services.

The highlights of Golem 1.2.2 include:

- **Golem LLM**. We released a new vendor-neutral standardized interface for Large Language Models (LLMs), with durable implementations for OpenAI, Grok, Anthropic, and Ollama (with upcoming implementations for Bedrock and others).
- **Updated Language Tooling**. We've upgraded language support to the latest toolchain versions for JavaScript, TypeScript, Python, and Go, simplifying integration for diverse development teams.
- **Enhanced Application Manifest**. The Golem application manifest now supports defining plugin installations and APIs directly. Additionally, we've introduced a new dependency type that allows direct dependencies on other WASM components at build-time, even pulling these components from remote URLs. The first provided WASM component library using this approach is golem-llm.
- **CLI Improvements**. We've further streamlined our CLI, enforcing consistent naming conventions—each Golem component now shares its name with the WIT package it defines. We’ve significantly improved CLI error messages and added commands for conveniently managing dependencies directly from the terminal.
- **Golem Cloud Enhancements**. Project collaboration has become easier in Golem Cloud. Users can now reference accounts directly by email address, enabling effortless sharing and collaboration across teams.
- **Component-Level Environment Variables**. Developers now have the flexibility to define environment variables at the component level, rather than only per worker, enhancing configuration management.
- **Worker Forking**. Golem components have gained the ability to fork workers, allowing more sophisticated execution patterns and efficient parallel task management.
- **Rib REPL Integration**. Rib, our scripting language, continues to mature. Beyond its original role in the API gateway, Rib is now accessible through an integrated REPL within the CLI, providing developers an intuitive environment to interactively script and debug Golem workers.

Collectively, these refinements position Golem 1.2.2 as an even stronger platform for developing, deploying, and orchestrating reliable, complex agentic applications.

## Summary

Golem is shifting its focus towards agentic applications, leveraging its durable computing infrastructure to address the complex orchestration needs of AI agents and workflows. While the broader durable computing landscape is crowded, Golem's unique WebAssembly-based approach offers distinct advantages, including low-latency startup, secure sandboxing, and efficient resource usage. By concentrating on agentic services, Golem aims to overcome current WebAssembly ecosystem limitations and deliver a specialized solution for reliable state management, flexible control flows, and seamless integration with tools and APIs.

This strategic refocus involves investing in areas such as durable streaming, standardized APIs, tool usage and implementation, visual debugging, a high-level SDK, and a capability-based security model. These enhancements, coupled with expanded documentation and examples, will position Golem as the go-to platform for deploying robust and scalable agentic services. Golem's ability to provide non-invasive reliability, security, and observability makes it exceptionally well-suited for demanding agentic workloads.

For more information and resources dedicated to Golem's focus on agentic applications, visit our [new website](https://golem.cloud). Subscribe to the Golem newsletter to stay updated on our progress and developments in agentic orchestration.
