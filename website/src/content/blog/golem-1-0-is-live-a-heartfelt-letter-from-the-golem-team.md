---
title: "Golem 1.0 is Live: A Heartfelt Letter from The Golem Team"
date: "2024-08-23"
author: "Golem Cloud"
tags: ["Product Updates", "Durable Computing", "WebAssembly", "Open Source", "Serverless"]
slug: "golem-1-0-is-live-a-heartfelt-letter-from-the-golem-team"
originalUrl: "https://golem.cloud/post/golem-1-0-is-live-a-heartfelt-letter-from-the-golem-team"
---

## Introduction

A year and a half ago, in a cramped coworking space in the heart of the Scottish Highlands, I and other members of a new stealth team at [Ziverge](https://ziverge.com) sat huddled around a MacBook Pro.

We witnessed something so pivotal, I was recording the sight on my phone.

We had just watched as an ordinary program that we compiled to WebAssembly was forcibly terminated mid-way through execution.

The forcible termination wasn't the interesting part. The part that got us so excited was what happened after the termination.

We watched as some new open source software, codenamed Golem, brought back the failed program and restored its state to the moment before termination.

As if by magic, the program resumed exactly where it left off!

We had taken an ordinary program, and without any SDK or DSL, transformed it into one that was automatically fault-tolerant.

This early prototype became the heart of Golem, the world's newest entry into the exciting new space of durable computing.

Today, after a year and a half of work by one of the technically best teams I've had the pleasure of working with, we are excited to announce the release of 100% open source Golem 1.0.

## The Genesis of Golem

The idea for Golem was born after another open source project that Ziverge launched called [ZIO Flow](https://github.com/zio/zio-flow). Sponsored by a company in the insurance industry, this project was designed to bring a highly-reliable workflow engine to the ZIO ecosystem.

By highly-reliable, I mean a workflow engine that durably executes workflows (conceptually equivalent to continuous, whole-system snapshots), so that in the event of any restart, update, or fault, the workflow can be restored and resume activity where it left off.

Durable execution promises to give developers a bulletproof foundation for building highly reliable distributed systems–be they checkout workflows, financial transaction processing, stateful and long-lived AI agents, or just backend APIs that need to coordinate updates and activities across many systems.

It promises a massive reduction of architectural, infrastructure, and engineering costs, primarily because engineering this level of fault-tolerance atop commodity hardware generally implies adopting complex and behemoth event-driven architectures.

With ZIO Flow, we knew we were delivering something valuable, but after much deliberation, I concluded that ZIO Flow would never become mainstream.

The reasons were simple:

1. ZIO Flow has a DSL and SDK, which requires that you retrain your developers and rewrite to use ZIO Flow.
2. ZIO Flow is written in Scala, which is a niche programming language, without significant adoption in industry.

Despite ZIO Flow's niche market, the idea remained intriguing to me. But after completing the first version of the project, we parked it for months.

Sometime in late 2022, I started digging into WebAssembly (WASM) for its potential to simplify cloud-native development, deployment, and operations.

Not long thereafter, I got a crazy idea: could we implement durable execution not with an SDK written in a specific language, but with an execution engine for any program compiled to WASM?

In theory, this would allow developers in any programming language and with any technology stack (so long as it compiles to WASM) to "push a button" and get durable execution for free.

The idea that you could push a button, and now your running programs would automatically survive restarts, updates, and faults, with zero changes–and on commodity clouds, without the need for specialized hardware or virtualization–seemed magical.

Possibly too magical to actually exist.

So after some weeks, I put together a tiny team inside Ziverge to fly out to Scotland for a couple weeks of intensive hacking.

The rest is history. Or at least, in the process of becoming history!

## The Golem 1.0 Package

The launch event is today, August 23rd, 2024, which marks just over a year since we launched the Developer Preview of Golem.

The Developer Preview represented the *bare minimum* functionality necessary for developers to preview the technology.

Over the course of the developer preview, we gained early users, who built example systems on Golem, ranging from trading platforms to streaming analytics to campaign orchestration. We acquired a design partner, who worked closely with us to meet their needs on the platform.

From these early users and our design partner, we have incrementally matured and refined the developer preview into a package we believe is ready for production usage.

We are rolling this out into 1.0, and equipping it with guarantees appropriate both for production usage, as well as the early stage nature of the open source project.

In the next section, I will tour the major features of the 1.0 release.

### Highlights

As the home page says, Golem is a durable computing platform that runs serverless workers invincibly, impervious to faults, restarts, updates, and transient failures.

Let's break down each of these components in more detail:

- **Durable Computing**. There are plenty of great durable execution solutions available today, but they work at the level of SDKs and runtimes. Golem is a new way to do computing, which doesn't require that your programs use any particular language or SDK.
- **Platform**. Golem is designed for both deployment and execution of your applications, web services, and backends. As such, it is a fully-featured computing platform, which you can deploy using Docker and Kubernetes, in a public or private cloud.
- **Serverless Workers**. Golem doesn't need you to write servers or manually support protocols, only code. Through WASM magic, Golem can invoke your code directly, and you get free and custom APIs atop your code.
- **Invincibly**. Golem features automatic *durable execution* for all workers, providing strong transactional guarantees. Your running code automatically and invisibly survives restarts, updates, and faults (such as hardware and software failures).

These features imply a lot of power–much more than can be explored in this post. But together, they enable you to build highly reliable distributed systems with impossibly simple code.

Now let's take a look at some of the features we've managed to incorporate into 1.0.

#### Transactional Execution

Golem executes your workers transactionally, all the way from beginning to end. This guarantee holds even in the event of restarts, updates, and faults–including hardware failure, operating system failure, even power failure!

Transactional execution eliminates partial updates and inconsistent states, providing a robust foundation for building highly reliable distributed systems.

#### Durable State

Because Golem executes workers transactionally, it means that any data stored in memory is persistent. This includes local variables, in the context of code that is currently executing, global variables, and even which part of the code is executing.

Durable state provides a way to reduce your application's dependency on databases, key-value stores, and caches, because all in-memory data is as persistent as a database.

#### Reactive Workers

In other serverless platforms, "workers" are functions, which are invoked a single time. But with Golem workers are software components that are instantiated, with potentially many functions, and they can be invoked repeatedly and live as long as you need them to live.

Compared to "one-shot" workers, like lambdas, reactive workers allow much more sophisticated distributed systems to be built as pure code.

#### Long-Running Workers

Serverless platforms like AWS Lambda timeout long-running workers. Golem, on the other hand, can execute workers for milliseconds, days, or even years–reliably and without loss of progress, state, or any data.

Golem's support for long-running workers makes it easy to build and deploy workflows on the platform, including business process automation, ETL, report generation, stateful AI orchestration, user onboarding, and many other long-running business processes.

#### Horizontal Scalability

Golem shards worker execution across any number of nodes for horizontal scalability. Workers that are inactive, due to lack of use or because they are scheduled to activate in the future, are suspended and moved out of memory to conserve CPU and memory.

Golem's built-in support for horizontal scalability, as well as easy deployment using Kubernetes, make it possible to solve the largest challenges.

#### Worker-to-Worker Communication

Most cloud systems interact with each other through protocols like HTTP and gRPC. Golem instead allows workers to directly communicate with others in a type-safe way, without the need for JSON or gRPC serialization.

Worker-to-worker communication lets you perform internal communication across different stateful workers, without having to build all of the traditionally required protocol boilerplate.

#### Reliable Communication

Golem has two separate mechanisms for reliable communication: for communication within Golem (worker-to-worker), Golem guarantees reliable, exactly-once invocation, without possibility of failure. For external communication, Golem supports idempotency keys (which provide exactly-once semantics for APIs that support them) and automatic retries for transient failures.

By taking care of making communication reliable, including supporting exactly-once semantics (automatically for internal communication, opt-in for external), Golem makes it much easier to build highly reliable distributed systems.

#### Custom APIs

Golem has built-in support for triggering workers from HTTP events, but there are many scenarios where these "automatic APIs" are insufficient. To make it easy to create custom APIs, Golem allows you to bind routes in OpenAPI definitions to workers, using a lightweight scripting language called *Rib* to do any data massaging.

With support for custom APIs, Golem lets you deliver any API that you want for front-end teams or third-parties, without having to contaminate your business logic with HTTP protocol code.

#### Live Update

Golem can update workers as they are running to a newer version of their code, which is useful to fix bugs or add features to long-running (or infinitely running) workers.

## Guarantees

Golem is an early stage open source project. Although we have done our best to learn from early users and incorporate this feedback into the project, we know we will have missed some important features, and that the architecture of the project will continue to improve.

Yet, given that one of the primary use cases for durable execution platforms like Golem is long-running workflows, we feel we need to provide some guarantees that will encourage early adoption of Golem 1.0 in mission-critical use cases.

So we are providing the following guarantees with the Golem 1.x line:

- **Serialization Stability**. If the format used to persist the state of workers changes during Golem 1.x, it will change in a fully backward compatible way, such that newer versions of Golem 1.x are able to decode state that has been serialized in older versions.
- **API Stability**. If the REST API for Golem changes during Golem 1.x, it will change in such a way that applications written to the old version will continue to work with the new version, with the only exception being the API for API Definitions.
- **WASM Stability**. Golem 1.x will continue to support WASI preview2 and the Golem host functions available at the launch of 1.0, and if there are any changes, they will be purely additive and explicitly marked experimental to warn users against depending on them.
- **Bug Fixes**. The 1.x line will continue to receive bug fixes until at least February 23rd, 2025. This means a minimum of 6 months of continuous support before the next major version of Golem is released.

In addition to these guarantees, we will provide best effort to keep backward compatibility for components outside the core of Golem, including Rib and custom API definitions.

For an early stage project, these guarantees mean you can build and deploy a wide variety of projects on Golem with the confidence that you will have extensive backward compatibility and bug fix guarantees well into the future.

## Roadmap

Golem 1.0 is the starting point of a journey, not the end. Although the actual set of improvements that are made to the project depend on user feedback and third-party contributors, there are a number of different areas we plan to focus resources on in the coming months.

Some of the most important focus areas are as follows:

- **Databases**. We will support database interactions directly, without requiring that users interact with databases through HTTP. This direct database support will leverage idempotency keys to provide exactly-once semantics.
- **Developer Experience**. We will improve the developer experience. These improvements range from more documentation and examples, to simpler ways of specifying and building components, to improved error messages and enhanced tooling.
- **Language Support**. We will improve support for more mainstream languages, aiming to bring Typescript and Javascript into our fully supported tier, and improve support for existing languages such as Go.
- **Permissions**. We will introduce a compositional permissions system that works within a Golem cluster, and can be used to make sure that workers can only communicate with other workers if they have the necessary permissions.
- **Rib Enhancements**. We will enhance Rib to support more nuanced type inference, data massaging over lists and other composite types, and other features as necessary to make the job of defining custom APIs simple and reliable.
- **Sharding Manager**. The shard manager is a component of Golem that is responsible for deciding which worker executor nodes are assigned which workers. Currently, the shard manager is a single point of failure, which can increase recovery latency in the event of a failure of the shard manager. We plan to invest resources in addressing this issue, as well as providing more flexibility on how workers are assigned to executor nodes.
- **Improved Golem API**. We will expose all of Golem's functionality in APIs you can call from any language (*host functions*), as well as expose new useful functionality such as scheduling, which you would have to implement yourself today.
- **Cloud Improvements**. We currently have only rudimentary support for our managed version of Golem Cloud, intended only to help developers get up and running with solutions faster than self-hosting permits. We will invest more heavily in bringing Cloud to a point where we can offer paid hosting plans.

If you experiment with Golem and find you need some feature in order to go into production, then please just reach out to myself or any of the core Golem contributors.

## Learning More

Later on today (August 23rd), at 12 noon Eastern Time, we are planning a special launch event, where we introduce Golem and give you a taste of its power for solving complex problems.

Attending the launch event also enables you to participate in a Golem Hackathon, scheduled for August 30th, where you can win more than $5k in cash by building cool applications on Golem.

Beyond attending the demo, if this post has got you excited about Golem, then be sure to check out the following resources:

- [Golem 1.0 Release](https://github.com/golemcloud/golem/releases/tag/v1.0.0)
- [Golem Documentation Center](https://learn.golem.cloud/)
- [Golem Discord](https://discord.gg/UjXeH8uG4x)
- [Durable Computing on YouTube](https://www.youtube.com/@DurableComputing)
- [Golem Newsletter](https://www.golem.cloud/)
- [Golem Cloud Website](https://www.golem.cloud/)

Cloud computing has transformed software engineering, triggering a cascade of change that has left many developers struggling with the complexities of building highly reliable distributed systems.

We believe the answer to the complexity and limitations of cloud computing is the simplicity and power of durable computing.

Along with other solutions in the space, we believe that Golem will lead to a massive shift in the way developers build reliable distributed systems.

We hope you are as ready for this future as we are!

Join the Golem community on [Discord.](https://t.co/LvB3ymIH1t)
