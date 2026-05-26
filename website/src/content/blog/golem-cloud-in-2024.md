---
title: "Golem Cloud in 2024"
author: "John A. De Goes"
tags: ["Product Updates", "Serverless Computing", "Durable Computing", "Open Source"]
slug: "golem-cloud-in-2024"
originalUrl: "https://golem.cloud/post/golem-cloud-in-2024"
date: "2024-12-09"
# date sourced from site-deploy timestamp "Mon Dec 09 2024" embedded in first wayback snapshot of post (web.archive.org/web/20250117022712/https://www.golem.cloud/post/golem-cloud-in-2024); coincides with Golem v1.1.0 release date and year-in-review style title
---

Happy New Year to all the worldwide fans of durable computing, and to all those following the latest open-source innovation from [Ziverge](https://ziverge.com), the team that brought the world [ZIO](https://zio.dev)!

In August 2024, and after a year and a half of development, we took a major step forward in our journey as we released the first version of [Golem](https://github.com/golemcloud/golem).

In this post, I want to talk about the year behind us, and take a peek into the year ahead of us–and what better way to do both than by talking about what Golem *is*.

## What is Golem

A new open-source serverless computing platform, Golem brings elasticity and reliability together in a robust offering that enables organizations to efficiently apply serverless patterns to mission-critical systems.

The defining feature of Golem is its support for *transparent durable computing*. Simply stated, Golem gives you "push button" durability, which you can turn on and off for different components that you deploy to Golem.

Turn durability on, and you get incredible features like automatic fault-tolerance (where your running code gets migrated in real-time and without interruption if there is a server or hardware issue), transactional code execution (which lets you build workflows that reliably execute across weeks or even years), and durable state (which lets you ditch your database and just store critical information in memory).

Turn durability off–for services that don't require it–and you save on storage, network, and CPU.

Golem's *software-defined reliability* opens up serverless to a whole new gamut of applications that could *never* run on AWS Lambda or Cloudflare Workers–reliable applications, stateful applications, long-running applications, and distributed applications.

## Key Milestones in 2024

A lot happened in 2024, both before the release and after. But looking back over the year, a few key milestones stand out:

- **Visibility**: The Golem [open-source project](https://github.com/golemcloud/golem) rapidly grew to 624 stars. Our [Discord](https://discord.gg/UjXeH8uG4x) has picked up more than 450 developers. Golem [first appeared](https://www.thoughtworks.com/en-us/radar/platforms/golem) on ThoughtWorks Technology Radar.
- **Market Fit**: We picked up our [first design partner](https://www.usepara.com/), who worked closely with us on a prototype of a key component of their marketing automation platform. Out of this collaboration has come a number of improvements scheduled for development.
- **Contributions**: We received our first significant open-source contributions from third-party members of the community.
- **Development**: We launched Golem 1.1 with new critical features, including ephemeral workers, plugins, robustness improvements, user-defined initial files for workers, better support for CORS and authentication, and more.

Yet, despite the fact that 2024 was such a significant step for the future of durable computing, it's impossible for me to shake the feeling that 2025 is going to be bigger, better, and far busier!

## What's Ahead in 2025

In getting Golem 1.0 out the door, we focused on the minimum features required for production usage. In doing so, we intentionally neglected two other aspects of Golem:

- **Developer Experience**. Golem 1.0 is rough around the edges, requiring a number of steps to build and deploy components.
- **Language Support**. Golem 1.0 focused its language support on WASM-friendly languages, most notably Rust and Go (through TinyGo).

As the old adage goes, "make it work, make it pretty, make it fast".

In 2024, we made Golem work. In 2025, we're going to make it "pretty" – which in our case, means improving developer experience and language support.

These improvements broadly fall into the following categories:

- **Build & Deployment Experience**. Even in Golem 1.1, the release we managed to ship just before Christmas, we have improved the build & deploy experience, but we have much farther to go – removing toolchain requirements, improving error messages, providing more and better starter templates, and simplifying or removing steps.
- **Development Experience**. Golem 1.0 only supported HTTP functionality, which required that developers interact with external systems through the HTTP protocol. We want to make serious progress supporting direct database access and high-level interaction with APIs defined by protobuf (gRPC), OpenAPI, Smithy, and GraphQL.
- **Language Support**. We want to improve our existing experimental support for Javascript/TypeScript to production quality, and experiment with Kotlin and the .NET ecosystem, which are both investing heavily into WASM. We also hope to see improvements in Python and Go support.
- **Ecosystem**. Our vision for Golem is to be small, secure, robust, modular, and highly extensible, with most "platform functionality" actually produced by third-parties. We want to start cultivating a vibrant ecosystem where add-ons (such as OTel, authentication, etc.) enrich the core platform experience, providing a batteries-included experience for developers that helps them focus on business logic.

Separately, we want to invest in launching a cloud-hosted version of Golem that provides additional operational and troubleshooting capabilities, integrations with existing protocols and services, and some key improvements to architecture that are important for multitenancy.

With these improvements in developer experience and language support, as well as enhancements to feature set, we are aiming to go from "possible" to "joyful" for developers, and from "evaluate" to "adopt" for organizations.

## Summary

The year 2024 marked Golem Cloud's emergence with its August 1.0 release, introducing transparent durable computing to serverless architecture. The platform gained significant traction, reaching over 600 GitHub stars and 450 Discord members, while version 1.1 brought essential features like ephemeral workers and plugins.

Looking ahead to 2025, Golem shifts focus from "making it work" to "making it pretty" by enhancing developer experience and language support. The roadmap includes streamlined deployment, expanded API protocols beyond HTTP, improved JavaScript/TypeScript support, and a new cloud-hosted version.

Through these improvements, Golem aims to transform from merely "possible" to "joyful," making durable computing accessible for applications that traditional serverless platforms cannot support.
