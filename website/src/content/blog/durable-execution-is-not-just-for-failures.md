---
title: "Durable Execution Is Not Just for Failures"
date: "2025-03-28"
author: "Daniel Vigovszky"
tags:
  [
    "Durable Execution",
    "Golem",
    "Worker Management",
    "Memory Management",
    "WASM",
    "Cloud Computing",
  ]
slug: "durable-execution-is-not-just-for-failures"
originalUrl: "https://golem.cloud/post/durable-execution-is-not-just-for-failures"
---

*Posted on March 28, 2025*

## Introduction

When talking about [Golem](https://golem.cloud/) or other **durable execution engines** the most important property we are always pointing out is that by making the application *durable*, it can automatically survive various failure scenarios. In case of a transient error, or some other external event such as updating or restarting the underlying servers durable programs can survive by seamlessly continuing their execution from the point where they were interrupted, without any visible (except for some latency, of course) effect for the application's users.

But having this core capability has many other interesting consequences.

A durable program can be dropped out of memory any time without having to explicitly save its state or shut it down in any way - and whenever it is needed it can be automatically recovered and it continues from where it left. The application developers can rely on very simple code storing everything in memory - as it is guaranteed that the in-memory state never gets lost.

If a **Golem worker** (a running durable program) is not performing any active job at the moment - for example it is waiting to be invoked, or waiting for some scheduled event - they automatically get dropped out of the executor's memory to make space for other workers. This means we can have an (almost arbitrary) large number of "running" workers, if they are not performing CPU intensive tasks. Sure, having to continuously recover dropped out workers is affecting latency, but still, it means we can run these large number of simultaneous, stateful programs even on a locally started Golem on a developer machine.

## Demo

### Setting it up

In this short blog post we are going to demonstrate this. We are going to start the latest version of Golem (1.2) locally, then use the CLI (and some [Nushell](https://www.nushell.sh/) snippets) to build, deploy and run a large number of workers.

First we download the latest `golem` command line application [according to Golem's Quick Start pages](https://learn.golem.cloud/quickstart). With that we can start our local Golem cluster - all the core Golem services are integrated in this single `golem` binary:

```bash
golem server run
```

We are going to use the same `golem` CLI application to create, deploy and invoke Golem components.

Next we create a new *golem application*:

```bash
golem app new manyworkers rust
```

![](/blog-images/68d76ba7edec7ec0b5c055af_67e7f21355aabe966624fd28_unnamed.png)

Golem comes with a set of **components templates** for all supported languages. One of these templates is a simple *shopping cart* implementation in Rust, where each Golem worker (running instance of this component) represents a single shopping cart, keeping its contents in memory.

We are going to create **10** (identical) versions of this template, simulating that we have more than one applications running in a cluster. Even though they are going to be exactly the same to keep the post simple, from Golem's point of view it is going to be 10 different applications, compiled and deployed separately.

Let's call the `golem component new` command 10 times in the newly generated application to set this up!

```nushell
0..9 | each { |x| golem component new rust/example-shopping-cart $"demo:cart($x)" }
```

This command created 10 components in our application, with names `demo:cart0` to `demo:cart9`. First let's build and deploy these components:

```bash
golem app build
golem app deploy
```

![](/blog-images/68d76ba7edec7ec0b5c055b5_67e7f21355aabe966624fd3d_unnamed.png)

To see the interface of this example, let's query one using `component get`:

```bash
golem component get demo:cart0
```

![](/blog-images/68d76ba7edec7ec0b5c055bb_67e7f21355aabe966624fd46_unnamed.png)

Before spawning our thousands of workers, we try out this exported interface by creating a single worker of `demo:cart0` called `test` and calling a few methods in it:

```bash
golem worker invoke demo:cart0/test initialize-cart '"user1"'
```

![](/blog-images/68d76ba7edec7ec0b5c055b4_67e7f21355aabe966624fd25_unnamed.png)

```bash
golem worker invoke demo:cart0/test add-item '{ product-id: "p1", name: "Example product", price: 1000.0, quantity: 2 }'
```

![](/blog-images/68d76ba7edec7ec0b5c055ba_67e7f21355aabe966624fd2b_unnamed.png)

```bash
golem worker invoke demo:cart0/test get-cart-contents
```

![](/blog-images/68d76ba7edec7ec0b5c055b1_67e7f21355aabe966624fd2e_unnamed.png)

For some more context, we can also check the size of the compiled WASM files (we were doing a debug build so they are relatively large) for these components:

![](/blog-images/68d76ba7edec7ec0b5c055b7_67e7f21355aabe966624fd49_unnamed.png)

We can also query metadata of the created worker to get the same size information, and it is also going to tell us the amount of **memory** the instance allocates on startup:

```bash
golem worker get demo:cart0/test
```

![](/blog-images/68d76ba7edec7ec0b5c055b8_67e7f21355aabe966624fd40_unnamed.png)

And we can query the test worker's *oplog* to get an idea of how much additional memory it allocated dynamically at runtime:

```bash
golem worker oplog demo:cart0/test --query memory
```

![](/blog-images/68d76ba7edec7ec0b5c055b2_67e7f21355aabe966624fd31_unnamed.png)

### Spawning many workers

Now that we have seen how a single worker looks like, let's spawn 1000 workers of each test component. This is going to take some time as it actually **instantiates** the WASM program for each to make the initial two invocations.

```nushell
mut j = 0;
loop {
    mut i = 0;
    loop {
           golem worker new $"demo:cart($i)/($j)";
           golem worker invoke $"demo:cart($i)/($j)" initialize-cart '"user1"';
           golem worker invoke $"demo:cart($i)/($j)" add-item $"{ product-id: \"p1\", name: \"Example product ($j)/($i)\", price: 1000.0, quantity: 2 }";

           if $i >= 9 { break; };
           $i = $i + 1;
    }
    if $j >= 999 { break; };
    $j = $j + 1;
}
```

After that, we have 10000 "running" workers (all idle, waiting for a next invocation). We can check by listing for example one of the component's workers:

```bash
golem worker list demo:cart5
```

![](/blog-images/68d76ba7edec7ec0b5c055b3_67e7f21355aabe966624fd43_unnamed.png)

Of course only some of these workers (the last accessed ones) are really in the locally running executor's memory. Whenever a worker that's not in memory is going to be accessed, it is loaded and its state is transparently restored before it gets the request. Golem is tracking the resource usage of its running components and if there is not enough memory to load the new component, an old one is going to be dropped out.

### Trying it out

To demonstrate this, we can just invoke workers randomly from the 10000 we've created:

![](/blog-images/68d76ba7edec7ec0b5c055b9_67e7f21355aabe966624fd4d_unnamed.png)

Thanks to the durable execution model, every one of the 10000 workers reacts just as if it was running.
