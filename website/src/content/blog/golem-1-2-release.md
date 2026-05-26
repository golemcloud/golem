---
title: "Golem 1.2 Release"
date: "2025-04-03"
author: "John A. De Goes"
tags: ["Product Updates", "Announcements"]
slug: "golem-1-2-release"
originalUrl: "https://golem.cloud/post/golem-1-2-release"
---

# **Golem 1.2 Release**

_On April 3rd, and coinciding with our official participation at KubeCon in London (April 1 - 4), we are excited to announce the release of Golem 1.2, together with a massive overhaul of the Golem website._

The pervasive theme of the Golem 1.2 release is **developer experience**. Though Golem has been fully operational since 1.0, we initially prioritized the minimum viable feature set above everything else.

With the 1.2 release, we are bringing a focus on developer experience to the forefront, and while we still have a lot more work to do, we have taken giant steps toward a future in which building for Golem is delightful on every level.

Our major improvements in this release fall into the following categories:

- **Golem Desktop**
- **Golem CLI**
- **Golem Server**
- **Worker-to-Worker Communication**
- **Platform**
- **Operations**
- **Worker Gateway**
- **Rib**

In the sections that follow, I will discuss these improvements in some depth.

## **Golem Desktop**

Golem Desktop is a new desktop-based application designed to help you get started quickly using Golem.

The application has many features from Console, but rather than requiring Golem Cloud (our in-development hosted version of Golem), Golem Desktop works with a local Golem server.

Desktop is 100% open source and has a full suite of features, including:

- **Manage components & workers**
- **Invoke functions on workers**
- **Build and test REST APIs for triggering workers**
- **Diagnose problems**

_The following screenshots will give you a feel for the depth and breadth of what Golem Desktop delivers for Golem developers._

_If you wish to download or Zoom into them for a better viewing experience, look at the screenshots_ [_here._](https://drive.google.com/drive/folders/1bpc1CfrqFxbp3lzczVvWSUBfdGiCOklg?usp=sharing)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055ca_67ee55fbcc031a429bfacf41_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055c7_67ee55fbcc031a429bfacf3b_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055c6_67ee55fbcc031a429bfacf50_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055c9_67ee55fbcc031a429bfacf3e_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055cb_67ee55fbcc031a429bfacf44_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055cd_67ee55fbcc031a429bfacf53_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055c5_67ee55fbcc031a429bfacf35_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055c4_67ee55fbcc031a429bfacf59_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055cc_67ee55fbcc031a429bfacf4d_unnamed.png)

![](https://cdn.prod.website-files.com/68d76ba7edec7ec0b5c05532/68d76ba7edec7ec0b5c055c8_67ee55fbcc031a429bfacf38_unnamed.png)

## **Golem CLI**

The Golem command-line interface, initially just a thin wrapper atop the server's API, is now becoming the centerpiece of Golem application development, with rich features designed to streamline both development and deployment.

We've invested heavily in improving usability of the CLI, including aligning the feature set around typical development lifecycles.

Among the improvements, you will find:

- **Streamlined command hierarchy.** The new command hierarchy revolves around defining an application as one or more components, and supports directly deploying either individual components or the whole application.
- **Actionable errors.** With some effort, many commands now provide detailed, actionable error messages. We love Rust's amazing compiler error messages and are starting to bring some of this magic to Golem CLI.
- **New composable templates.** Golem CLI has long been able to create example components from built-in templates. But now these templates all use the new manifest file (a declarative specification of a component), and they all work together, allowing easy creation of cross-language applications.
- **Lenient input.** Golem CLI allows shorthand, including omitting component identities where they can be inferred, using slugs (\<component-name\>/\<worker-name\>) instead of component identities, using a subset of fully-qualified function names (e.g. omitting package). This has a profound effect on how pleasant it is to use the CLI for day-to-day development.
- **CLI unification.** Rather than have two separate CLIs, one for self-hosted (and local) Golem, and one for our developer preview, there is a single CLI application that works seamlessly across both.
- **More integrated manifest.** Many commands now use the manifest file, and feature much tighter integration with it. Over time, the CLI will evolve to fully fit a developer's workflow, rather than reflecting the server's API.

The CLI has rapidly become a highly polished tool for building, deploying, and troubleshooting components, with many more improvements right around the corner, such as a REPL.

## **Golem Server**

The single executable (a version of Golem server that bundles everything into a single program) now runs natively on Windows, not just Mac and Linux. Please let us know if you run into any issues with this new OS support!

## **Worker-to-Worker Communication**

Benefiting from both type-safety (even across languages) and exactly-once communication semantics, worker-to-worker communication is a core feature of Golem, immediately useful to replace message buses and other event-oriented architecture.

However, worker-to-worker communication has proven a persistent pain for users to get working correctly.

To improve the situation, we worked on the following enhancements:

- **Dynamic stubs.** Previously, to use worker-to-worker, you had to generate and compile stubs written in the Rust programming language. These steps have been removed entirely. Golem CLI generates the IDL files (in WIT format) necessary to do worker-to-worker communication, and these interfaces are stubbed dynamically in the Golem server at runtime.
- **Simpler RPC constructors.** Previously, in order to use worker-to-worker communication, you had to identify the target component by its UUID. The only problem is that UUIDs are not known until deployment time, creating an awkward usability problem. Now, you only need to specify the target worker name, and the component identity is looked up based on the name of the component, which is embedded into the dynamic stub.

There are further enhancements we will make to worker-to-worker communication, but already, these improvements have a large effect on making the feature easier to use than ever before.

## **Platform**

The core platform, which provides seamless durable execution of WASM components (replicating state and handling recovery for faults, failures, restarts, and updates), has undergone a small number of focused improvements.

- **Custom durability.** Golem achieves durable execution by providing a durable implementation of core WASI (like POSIX) host interfaces, which are used by languages that support compiling or bundling to the WASM component model. Now, with the custom durability API, developers can make their own core WASI-like libraries durable. Though not something most developers will ever know about or use, this feature allows a lot of flexibility in customizing durability semantics for low-level libraries.
- **Invocation context.** Invocation context is a feature that allows Golem to seamlessly propagate context all the way from triggers throughout the distributed call graph (in worker-to-worker communication). This is the foundation of our upcoming support for OTel.
- **Simpler plugins.** Plugins are a powerful way to extend the functionality of Golem. Previously, we supported two types of plugins, transformer plugins, and oplog processor plugins, each of which has different capabilities. We recognized that transformer plugins, although powerful, are difficult to use when using them to supply some missing functionality to a Golem app (like a database interface). To rectify this issue, we introduced a new type of plugin that encapsulates this common use case. Golem will ship with its first built-in plugins in the next release.
- **RDBMS support.** In theory, one can simply compile any database clients to WASM and use them on Golem without changes. In practice, however, the WASM ecosystem is not yet mature enough to compile database clients without significant work to core open source libraries. To bridge the gap, we have introduced an extensive but lightweight component model interface to MySQL and Postgres, allowing Golem workers to use databases without an HTTP-based bridge.

Together, these changes simplify and extend current development capabilities, and prepare for future improvements to the platform.

## **Operations**

A big chunk of what a developer does comes after pushing the big deployment button, and this release contains a number of improvements targeting simplified operations:

- **Core debugging service.** The core debugging service will be used to introduce a time traveling debugger. This debugger will allow you to step through the historical action of a worker, and interact with it at that point in time, helping to diagnose, troubleshoot, and eventually repair issues that led to failure.
- **Core worker recovery.** Though not yet exposed in a high-level way, it is now possible to revert a worker to any point in time (including before failure), whereupon it will resume execution from that point in time. This will soon be exposed in the upcoming debugger GUI.
- **Cancel pending invocations.** A minor addition, this allows canceling any invocations on a worker which are pending.

Most of these improvements are at the level of core APIs, and not yet visible.

## **Worker Gateway**

The Worker Gateway is the part of Golem that sits between the outside world and workers executing in a Golem cluster. It handles routing, but also provides a way of adding custom APIs atop Golem workers.

This release brings a new type of binding called **WASI HTTP**. This type of binding is useful for components that implement the "lambda interface" for WASI, such as components built using Fermyon's Spin framework or wasmCloud's frameworks.

As WASM continues to gain traction in the serverless market, this new binding type will be critical in ensuring that Golem can run existing components that were built with third-party frameworks and libraries.

## **Rib**

Golem automatically exposes the public API of your components (defined using WIT, a Protobuf-like IDL). However, in many cases, you want to choose to craft a custom API for your backend, which allows triggering and interacting with running workers.

To simplify this process, Golem lets you define each endpoint using Rib, a lightweight, type-inferred scripting language that natively understands the types of the component model. In essence, Rib is the glue that lets you easily add custom APIs on your components, reshaping the inputs and outputs as dictated by business requirements.

Rib has gotten a wealth of improvements, most of them targeted at usability, and a few targeted at expressivity:

- **First-class workers.** Previously, one had to define the worker that a Rib script would interact with outside the script itself. But now, it's possible to create variables that refer to specific workers (e.g. `let worker = instance("shopping-cart-${username}");`). This not only simplifies development of custom APIs, but is a necessary step for allowing scripts to coordinate across multiple workers.
- **List comprehensions.** Rib now supports list comprehensions, which allow transforming and remapping the contents of lists. Previously, it was not possible to change the contents of lists, which forced developers to reflect details of their 'public API' in their private (WIT) API.
- **List reductions.** Rib also supports list reductions, which makes it possible for Rib scripts to turn lists (returned by workers, or provided as input to a REST API) into single values, allowing greater flexibility in the remapping process.
- **Better errors.** Many error messages in Rib have been improved, and all errors now come with source code locations.
- **Ranges.** Rib has first-class support for ranges (0..2), which allow iteration from within Rib scripts, greatly increasing expressive power and providing the ability to generate data, rather than just reshape it.
- **Inline type annotations.** Rib is fully type inferred, but in some cases, there is ambiguity. To better support these cases, Rib now supports inline type annotations, which can also be used to do coercions from path variables to numeric types.
- **Path variables string by default.** Rib previously had no default type for path variables. Yet most users expect path variables to be string by default, because an HTTP path is in fact a string. So now Rib infers path variables as strings.

See the Rib reference for further details on any of these features.

## **Summary**

Golem 1.2 delivers on our promise to make developer experience the priority.

We've introduced Golem Desktop for seamless local development, transformed our CLI to align with real developer workflows, and removed the headaches from worker-to-worker communication. The platform now offers custom durability, native database support, and improved operational tools that simplify debugging and management. With enhanced Worker Gateway compatibility and a more powerful, intuitive Rib scripting language, this release represents a significant leap toward our vision.

While our journey continues, Golem 1.2 demonstrates what happens when we put developers first. See it in action at KubeCon London, where we'll show how these improvements can transform your development experience.
