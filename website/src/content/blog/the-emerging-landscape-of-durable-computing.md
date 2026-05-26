---
title: "The Emerging Landscape of Durable Computing"
date: "2023-08-24"
# date sourced from site-deploy timestamp "Thu Aug 24 2023" embedded in first wayback snapshot of post (web.archive.org/web/20230922132236/https://www.golem.cloud/post/the-emerging-landscape-of-durable-computing); post absent from earliest blog snapshot 20230811 (which had 3 posts), present in /post/ wayback snapshot dated 2023-09-22
author: "John A. De Goes"
tags: ["durable computing", "cloud infrastructure", "workflow orchestration", "distributed systems"]
slug: "the-emerging-landscape-of-durable-computing"
originalUrl: "https://golem.cloud/post/the-emerging-landscape-of-durable-computing"
---

In a previous post, I answered the question, [What is Durable Computing?](/blog/what-is-durable-computing)

When you are deploying apps to a durable computing platform like [Golem Cloud](https://golem.cloud), you still have to worry about bugs in your code and external failures (such as failure of a database you are talking to), but you no longer have to worry about server failures or restarts.

When hardware or network fails, when a server fails catastrophically or is terminated by a supervisor (like Kubernetes), or when a server is restarted, durable computing platforms detect the failure event, and automatically migrate your program to another computer, restoring its state, and resuming execution transparently.

Historically, we would turn to event sourcing or other patterns to obtain the reliability and robustness that durable computing gives us for free. Code that should not work, like a simple script that handles the various steps of a transactional checkout flow in an e-commerce application, suddenly just works the way you would hope for.

Durable computing gives us a superpower to rapidly and simply build scalable cloud apps that are reliable and robust without having to 'engineer around' fragile cloud computing. In turn, this lets us deploy value to customers faster, more reliably, more robustly, and without needing any expertise in distributed systems engineering.

Perhaps these compelling advantages are why we have seen the market rapidly go from virtually no durable computing solutions to **dozens** in the span of a few short years. Even in 2023, we are seeing the launch of no less than 4 new entrants into the space, including [Golem Cloud](https://golem.cloud) (developer preview mode, anyway).

Every new entrant into the space makes a stronger case that, for at least specific use cases (if not all), we are on the verge of a re-invention of cloud computing. We are entering a new era in which decades of complexity are peeled back to reveal simple business logic.

In this post, I'm going to overview the landscape of durable computing solutions. To do this, we must consider a number of different dimensions that are relevant to users of durable computing, so we can gain insight into which solutions are a fit for which use cases.

### 13 Dimensions of Durability

I have identified thirteen different dimensions of durable computing that help us to characterize all the different solutions in the space.

These thirteen dimensions are as follows:

- **Granular**, which ranges from highly-granular to coarsely-granular. Some durable computing solutions provide highly granular durability guarantees inside active execution of a deeply nested call stack, at the level of every line of code or expression. Other solutions provide coarse-grained durability guarantees, typically between some steps of a high-level, orchestrated workflow. An advantage of highly granular durability is that everything is durable, so you don't need to think hard about **exactly where** durability guarantees apply. However, such durability might be more expensive (CPU/RAM), more restrictive, or more invasive than the alternative.
- **Reactive**, which ranges from reactive to imperative. Some durable computing solutions allow durably executing workers to react to external commands and queries (sometimes called event-driven workflows, entities, or actors), while other solutions allow only imperative, non-interactive workers, which execute from start to finish. Reactive workers are more flexible, as they facilitate applications involving durable state (such as shopping carts, gaming sessions, IoT control systems, and the like), but may be associated with higher cost (CPU/RAM) or boilerplate in accepting and processing queries and commands.
- **Immortal**, which ranges from immortal to mortal. Some durable computing solutions do not impose any restriction on how long your workers may execute. Whether they live for just milliseconds or whether they live for years, the same durability guarantees apply. Other solutions impose restrictions in order to manage the costs associated with durable computing. Although solutions that support immortal workers are more expressive (because when combined with reactivity, they open the door to entity-oriented use cases, like gaming sessions, shopping carts, user profiles, and so forth), they may be associated with higher cost (CPU/RAM) or additional restrictions or limitations in the way live code can be updated.
- **Updatable**, which ranges from updatable to fixed. Some durable computing solutions allow updating live code, which may be important across some use cases for workers that execute over long periods of time. Others do not allow updates because of the significant challenges involved in state migration. While updating is a desirable feature for long-running (or immortal) workers, it may impose constraints on the design of workers, or be associated with higher costs (CPU/RAM), and is not necessary for most applications of durable computing.
- **Automatically Durable**, which ranges from fully automatic to fully manual. Some durable computing solutions do not require that developers do anything in particular to gain durability: any code they write and any state they store is automatically durable. Other solutions do not provide automatic durability, and they require that developers be aware of durability and opt into durability for specific data or for specific parts of their code. Automatic durability provides an easier and, arguably, safer guarantee, but may be associated with higher costs (CPU/RAM) or more boilerplate, and sometimes having ephemeral state or non-durable sections of code is an acceptable to lower boilerplate or reduce costs.
- **Production Ready**, which ranges from production ready to concept stage. Some durable computing solutions are already proven in production, while others are at the prototype phase, and ready for exploration but not deployment. Still others are at the napkin phase, where code may exist, but there is no preview version for developers to try. Although for experimentation and prototyping, either type of solution is acceptable, developers looking to deploy immediately will need a solution that has been proven in production, and has both support as well as well-defined stability guarantees.
- **Vendor-Neutral**, which ranges from fully vendor-neutral to fully vendor-specific. Some durable computing solutions may not require any vendor-specific software development kit or toolset in order to execute, while others may require a (potentially open source) vendor-created and vendor-maintained toolset or SDK. Still others may require a proprietary SDK or toolset, which is closed source. For some companies, vendor-neutrality is important, while for others, it is irrelevant, especially if the vendor is a major cloud provider, who can be expected to provide long-term support.
- **Hybrid Deployment**, which ranges from hybrid deployment to just cloud deployment. Some durable computing solutions may be self-hosted by users, which enables on-prem and private-cloud use cases. Other solutions are only available as cloud-hosted offerings, which makes them easy to consume, but may limit use cases for companies that are not legally able to store or process sensitive information in the cloud.
- **Code-defined**, which ranges from fully code-defined, to fully configuration- or toolset-defined. Some solutions are not just code-first, but rather, code is used to build the entire application, with no reliance on configuration (such as JSON or YAML definitions) or graphical tools. Other solutions are primarily graphical, exposing a user-interface that can be used to construct data-defined workflows that can be executed durably. Developers tend to prefer code-defined solutions, because they integrate well into established developer tooling and are highly flexible. However, GUI solutions such as graphical workflow builders may offer the ability for less-technical people to be involved in the creation of key components of durable applications.
- **Multi-Language**, which ranges from multi-language to single-language. Some durable computing solutions are designed for a single language or single language family (for example, the JVM), while other solutions are designed for multiple languages across language families. Multi-language solutions offer broader appeal to developers from different backgrounds, but may not be able to take advantage of language- or runtime-specific features that can simplify the development, maintenance, and operation of durable applications.
- **Unobtrusive**, which ranges from unobtrusive to obtrusive. Some durable computing solutions allow developers to bring their own code, including data models, business logic, and other artifacts, and execute these directly by the durable computing solution. Others require developers to assemble their solution in a specific way, with care taken to map each piece to the correct underlying primitive that is used to power durability. An unobtrusive design helps developers leverage existing code assets and lowers barriers-to-entry, but may be associated with other drawbacks, including having to use special-purpose languages or platforms.
- **Low-Latency**, which ranges from low-latency to high-latency. Some durable computing solutions are designed for low-latency applications, such as payment processing flows. Others focus on higher-latency applications, including long-running workflows. Many applications of durable computing do not require low-latency, and while low-latency can enable certain new use cases, it may come at higher cost (CPU/RAM) and different technological approach.
- **Transactionality**, which ranges from full transactionality, achieving exactly once semantics with rollback on error, to zero transactionality, in which there is no rollback and any given step of computation might be executed any number of times. Although full transactionality would be desirable for some use cases, systems that offer full transactionality cannot invoke arbitrary cloud services (including microservices, REST APIs, and GraphQL APIs). Systems with full transactionality are closer to databases than to cloud computing platforms. A workable compromise for durable computing solutions is having exactly once execution semantics for local operations (including state updates), and at least once execution semantics for interactions involving cloud services, which permits retries in the event that a failure occurs while waiting for a cloud service to complete. With this guarantee, rollbacks can be implemented using the saga pattern for those cloud services whose actions can be effectively undone (for example, reversing a credit card charge).

### Landscape

I have identified more than a dozen different solutions in the space of durable computing. While not a comprehensive list, I believe these solutions adequately encompass the diversity of approaches to durable computing, and capture many of the notable players in the space.

I've grouped these solutions into incumbents, big tech solutions, challengers, and actor-based solutions.

**Incumbents**

The incumbents are major cloud computing players who have brought solutions for durable computing to market. These solutions generally do not represent the cutting-edge of what is possible, but they use well-studied and conservative approaches, they are proven in production, and they are backed by major players in the space of cloud computing.

- [Azure Durable Functions](https://learn.microsoft.com/en-us/azure/azure-functions/durable/durable-functions-overview). Microsoft has invested considerably into several different strategies for durable computing. In the case of Durable Functions, Azure provides a cross-language solution for defining orchestrations, which are executed durably and can interact with Azure Functions.
- [AWS Step Functions](https://docs.aws.amazon.com/lambda/latest/dg/lambda-stepfunctions.html). AWS Step Functions, built on AWS SWF (Simple Workflows), provides a serverless solution to durable computing, which leverages and integrates with the rest of AWS, especially lambdas.
- [Cloudflare Durable Objects](https://developers.cloudflare.com/durable-objects/). While not providing any kind of transactionality, Cloudflare Durable Objects can be used to implement lightweight forms of durable computing, especially where transactional guarantees are not required. As with other recent offers from Cloudflare, Durable Objects represent the company's commitment to enter cloud computing and do so in a way that provides tight integration with their edge network and the usability and reliability the provider is known for.

**Big Tech Solutions**

The big tech solutions had their origin inside tech companies, who lived with the pain of fragile computing for some time, before deciding they needed to create a solution to this problem. Some of these are open source, and others are commercializations of open source, but they all share in common a grounding in deep pains born from big tech, and they are all architected in such a way as to handle the scale of big tech.

- [Temporal.io](https://temporal.io). [Temporal.io](https://temporal.io) is both an open source project, as well as a company and cloud platform devoted to commercializing and supporting the open source project. Created by Uber engineers as a fork of its predecessor (Cadence), [Temporal.io](https://temporal.io) is the best funded and most commercially successful durable execution platform. The company has done a lot to establish durable execution as its own separate category, and for better or worse, many other solutions will be compared to [Temporal.io](https://temporal.io), even if they don't all target the same use cases.
- [Cadence](https://cadenceworkflow.io/). Cadence is an open source workflow orchestration engine originally created by Uber to handle their durable execution requirements. Some of the creators of Cadence went on to build [Temporal.io](https://temporal.io).
- [Apache Airflow](https://airflow.apache.org/). Airflow is an open source workflow orchestration engine originally created by Airbnb to handle the tech company's growing needs for durable execution. As with many solutions in the space, the orchestration itself is executed durably, even while individual steps that make up the orchestration are not durable.
- [Orkes](https://orkes.io/). Orkes is a company and platform built around the open source Conductor workflow orchestration engine, which was developed inside Netflix to handle their rapidly growing requirements for durable computing.
- [Conductor](https://conductor.netflix.com/index.html). Conductor shares a similar set of features to Orkes, but lacks many Enterprise features, which were built as closed source and are available exclusively through Orkes. However, the core feature set behind the solutions is very similar.

**Challengers**

The challengers are relatively new companies, most of which have formed in the wake of Temporal.io and adjacent big tech solutions, but which utilize different technological approaches and cater to different (but potentially overlapping) markets. Some challengers are open source, some are closed, and a number have raised venture capital. All challengers share a vision for a world in which durable computing is more pervasive and simpler.

- [Golem Cloud](https://golem.cloud). Golem Cloud is, of course, my company's own early-stage solution to the durable computing problem. Taking a radically different approach than other solutions, Golem is capable of durably executing any program without any modifications, so long as the program can be compiled to WebAssembly.
- [Statebacked](https://www.statebacked.dev/). Statebacked is the newest of several products that formed around [XState](https://xstate.js.org/), an open source project that allows building state machines. State machines have some beautiful properties for durable computing: the next step in their computation is always a deterministic function of their current state and the next input. Statebacked is leveraging these properties to provide a foundation for durable computing in Javascript.

- [Restate.dev](http://Restate.dev). Restate is one of several new Javascript-focused challengers built on the idea of durable async/await. Through its opt-in SDK, developers can choose which parts of their application to make durable, and deploy their applications on any commodity cloud platform.
- [Trigger.dev](https://trigger.dev/). Trigger is both an open source project and a company that is bringing durable execution to existing mainstream serverless computing platforms. By relying on a custom SDK for interacting with third-party services, Trigger allows TypeScript developers to build reliable long-running background jobs that execute even in environments where serverless functions are timed out.
- [Inngest](https://www.inngest.com/). Inngest is an open source project and company that augments serverless environments with primitives useful for durable computation, including durability for event-oriented multi-step processes such as workflows.
- [Resonate](https://github.com/resonatehq). Resonate is a new company formed by Dominik Tornow (ex-Principal Engineer at [Temporal.io](https://temporal.io)). Although not all details are known about what this company will develop, based on public comments and open source work, it appears Resonate will work toward a specification for durable promises, and implement a Go-based server that can durably execute await/async code in Javascript that is built on durable promises.
- [Effectful Technologies](https://www.effectful.co/). Effectful is a new company formed around the popular Effect TS open source ecosystem. Founder Michael Arnaldi has publicly expressed his ambition to bring durable computing to all TypeScript / JS users, even those not explicitly using Effect TS.

**Actor-based**

Before durable computing became a recognized space, there were actor-based systems that offered both distribution and persistence, which enabled them to tackle use cases requiring durable execution, durable state, and high scalability. At least one of these (Kalix) has emerged from actors to offer a new way to build microservices.

- [Orleans](https://github.com/dotnet/orleans/). Orleans is the product of Microsoft research into providing a new way to build highly scalable distributed, stateful applications. Based on actors, and with options available for persistence (including event-sourcing), Orleans is a very comprehensive take on what a distributed actor framework can look like.
- [Akka](https://akka.io). Akka is one of the most famous frameworks for implementing the actor paradigm, made by the Lightbend corporation. Actors are well-studied in distributed programming, and thanks to event-sourcing and snapshotting, they have a wide range of applications in durable computing, as well. Akka is no longer open source, but via the Apache Pekko fork, there is a way to access earlier (open sourced) versions of Akka.
- [Kalix](https://www.kalix.io/). Kalix is a "stateful serverless" solution, formerly branded Akka Serverless, produced by the Lightbend corporation. Kalix provides several layers that are useful for durable computing, including durable entities and reliable workflows, which are powered by Akka technology, and managed in the cloud.

### Visualization

Based on research I conducted across many of these solutions, I put together a visualization that highlights where different solutions fall on the thirteen dimensions of durability.

The dimensional values have the following meanings:

- **0**. No specific support for this feature at this time.
- **1**. Moderate but incomplete support for this feature.
- **2**. High support for this feature.
- **?**. Unknown support for this feature.
- **\<n\>**\*. Planned level of support for this feature at launch.

If you spot any inaccuracies, or have publicly disclosed data which could help fill in the gaps, then please reach out.

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055da_67559f0f16c5f7501359f4e0_13%252520Dimensions%252520of%252520Durability%252520-%2525202023-1.png)

### Summary

Using "fragile" clouds for all use cases forces developers to manually build robustness atop fragility, and while there are well-known techniques like event sourcing which can achieve this goal, it's a lot of work for something that shouldn't even be necessary.

To simplify these use cases, more than a dozen companies, products, and open source projects have formed around the idea of durable computing. These companies and projects make it easier than ever for developers to experiment with durability, and see whether or not the new paradigm can simplify the development of cloud apps.

Given there are no standards in the space, and that different solutions often target different use cases, it's important to be able to discuss the ways in which durable computing solutions are different. Through research and growing familiarity with this space, I've identified 13 different dimensions of durability.

Each solution optimizes for different tradeoffs along these dimensions, creating a highly diverse and competitive market. No one solution can be the best choice for all use cases, so the diversity in choices allows developers to choose the best solution for each situation. It is quite likely many of these solutions will end up owning some use cases.

It will be fascinating to watch the space of durable computing evolve, and it's my hope that by 2024, an updated "landscape of durable computing" will be authored and maintained by analysts or venture capitalists.

Until then, this post will have to do!
