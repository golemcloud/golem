---
title: "An Evening With Golem"
author: "Kamau Muiruri, Sarah O'Toole, Neal Milstein, Daniel Roy"
tags: ["Durable Computing", "WebAssembly", "Serverless", "Cloud Computing", "Golem Platform"]
slug: "an-evening-with-golem"
originalUrl: "https://golem.cloud/post/an-evening-with-golem"
date: "2023-12-04"
# date sourced from site-deploy timestamp "Mon Dec 04 2023" embedded in first wayback snapshot of post (web.archive.org/web/20231207211101/https://www.golem.cloud/post/an-evening-with-golem)
---

_This article is authored by Hivemind Technologies._

Twice a year at [Hivemind](https://hivemindtechnologies.com/), we hold a "FedEx Day", where – as with the delivery service namesake – we attempt to ship software in a day. We usually explore an interesting challenge that delves into new technologies, stacks, or programming languages to keep our tools sharp and have some fun.

This time, we played around with a technology still in its infancy: Golem, a cloud platform in development at Ziverge that boldly promises to turn the way we think about designing and building cloud applications on its head.

### A Different Approach To Solving Enterprise Problems

Golem is a serverless computing platform with a radical premise: your code will never fail due to an outside event – or at least, it will pick up right where it left off if it does. Every piece of code running in Golem comes with the guarantee that if some unexpected fault occurs (a hardware failure, a power outage, a stray cosmic ray), your code will resume in the exact state it was in before.

This approach is called "[durable computing](/blog/what-is-durable-computing)" and its advocates believe it will serve as a replacement for the retry mechanisms, latches, and multitude of other systems we have written into systems since the dinosaurs roamed the earth. Whereas applications today are considered "volatile" or "fragile" in their capacity to fail at any moment, durable applications have their recovery mechanisms baked into the platform.

### So What's So Special About Durable Computing?

Let's look at a sample workflow of an e-commerce application to make this more concrete:

1. Order submission
2. Payment transaction
3. Dispatch to the warehouse for shipment
4. Shipment scheduling
5. Product shipment to the client

What happens if there is an infrastructure failure right after the payment transaction, but before the order is dispatched to the warehouse? We can imagine the result – paying for items but receiving no goods – would lead to unhappy clients and lots of support calls.

One solution would be to use Kafka to create an immutable sequence of events that can be replayed in the event of failure. To do this, we would have to integrate events, error management, and replays into our business logic, turning this simple workflow into a much more complex application. Durable computing instead utilizes checkpointing to preserve state and resume any operation exactly where it left off. Golem units of execution are best described as serverless workers that ["continue to execute even if the node they are running on is downed due to hardware failures, updates, or connectivity issues"](/blog/what-is-durable-computing).

Golem Cloud promises to achieve this by ["continuously snapshotting the state of the workers"](/blog/what-is-durable-computing) to determine the exact position your code is currently executing at a given moment in time.

### Neutrality In Adoption

Golem takes a different approach to durable computing compared to other providers, especially those of the Workflow Engine kind. Most durable computing frameworks require using a specific language with a limited set of features. Golem instead embraces the WebAssembly specification, supporting any language that can produce server-side WebAssembly (WASM). This opens the door to a host of modern general-purpose languages, including Rust, Go, and Python.

WebAssembly is a specification for portable machine code that was originally designed to run in the browser. WebAssembly is relatively new and its adoption has been slow, but Golem deliberately chose to use it due to its locked-down security model. The same characteristics that make it ideal for the browser have made it a great fit for the Cloud.

Efficiently making snapshots of a program's memory requires controlling the program's memory and interactions with the host, something WebAssembly excels at. WebAssembly programs can only use functionalities provided by the host, and they can run in custom runtimes that alter and virtualize the program's environment. Compared to a runtime such as the JVM, WebAssembly presents a much more locked-down model that can be securely isolated and sandboxed.

### Installing And Running Applications On Golem Is Straightforward

Running applications in the Golem Cloud is straightforward, and mimics deploying code to other serverless providers. Golem provides the Golem Client for easily deploying new templates, and spawning workers.

[The quickstart](https://www.golem.cloud/learn/quickstart) is a good place to begin with your Golem journey and is what Hivemind followed during its FedEx day experiment with Golem, following the Rust example project.

To deploy to Golem Cloud, you must build a template using any programming language and toolchain that can build WebAssembly components. Templates expose functions that can be called externally. The template must then be made available for execution by uploading it to the Golem cloud via the CLI. You can then execute the uploaded template by creating a worker, which represents a separate execution of your template.

Some developers also experimented with what Golem considers a "Tier 3" supported language, Zig. We produced a simple app that reads a JSON array of integers and returns the sum. It took some work to produce the WASM target using wasm-tools, and we had trouble debugging the app due to Golem's minimal feedback, requiring a great deal of trial and error. Golem has recently announced a Management Console UI that may make these tasks much easier.

### Potential Use Cases

Golem is still in its infancy, so it's difficult for us to envision specific use cases for our software. We nonetheless have come up with a few areas where we feel Golem could have a big impact:

**Backend for Frontend.** Because Golem workers can safely keep state in volatile memory for long periods of time without risk of failure, one use case would be storing session information in a worker that handles multiple requests for a given user. Such a system would no longer require using non-volatile storage such as a database to store a user's state.

**Batch processing.** We currently rely on systems such as Spark to batch-process large amounts of data. One of Spark's main draws is the ability to automatically recover from node failures. We can envision using durable workers in the future to mitigate this failure instead, thereby not having to rely on libraries such as Spark.

**Stream processing.** We currently rely on mechanisms such as Kafka's at-least-once and exactly-once semantics to make guarantees about our compute processes. Durable workers could allow us to ensure similar such guarantees in more general contexts, without relying on tools such as Kafka for these safeguards.

### Our Questions About Golem

Our team was excited about Golem's potential to dramatically simplify codebases and lead to a paradigm shift in how we think of distributed systems. However, we had a few questions about how we would integrate Golem into our systems.

Firstly, does Golem provide a minimum recovery time for workers in the event of a fault? If a node suffers a hardware failure, do we know for sure that it will pick up where it left off after X seconds? The difference between a downtime of a few milliseconds versus 10 seconds could be the difference between seamlessly handling a request and needing to mitigate downtime.

Much of the data we handle for clients is sensitive. Golem keeps snapshots of our applications' memory for fault-recovery; do we have any guarantees that these snapshots are stored securely, such as encrypted storage?

Golem promises to simplify workflows by no longer necessitating recovery systems for external faults. But won't we still need these systems to mitigate potential internal errors? To take from the E-commerce example, Golem claims that we no longer need a recovery mechanism if the process crashes between the "payment transaction" and "dispatch to warehouse" step. But what if, say, a library we use has a bug and goes into infinite recursion between these steps? The worker will still crash and cannot be recovered. Won't this require the same recovery systems in place as before?

Live updates of running Golem workers appear to be a difficult task. If the core model changes, then separate code will be required to transition the old state to the new state. This is an extra complication.

### Potential Future Features

We had some ideas for potential future Golem features:

1. **Integration with other services.** One reason for AWS Lambda's popularity is its ease of integration with the rest of the AWS ecosystem. In the future, we would need similar such tools for any WASM application to integrate with Kafka, databases, etc. Right now, for a WASM app to communicate with Kafka for example, it would have to go through a REST proxy. Though one of Golem's selling points is using it to replace Kafka, that would for now require dramatically rewriting our existing apps.

2. **The ability to simulate failures with the [golem-cli](https://www.golem.cloud/learn/golem-cli).** Our team enjoyed using Golem as a serverless cloud provider, but it was difficult to validate Golem's uptime claims without being able to simulate external failures. Doing so would allow us to test how responsive Golem is and how our own app responds to these reboots

3. **A thorough example of the Promise system** to demonstrate asynchronicity, workers spawning new workers to split workloads.

4. **Golem Worker and snapshot introspection.** We thought it would be valuable to be able to see how Golem manages snapshots and worker state. Being able to inspect things such as the state, events, etc. would help us debug problems in our templates. We thought a graphical dashboard for tracking worker health and easy capturing failed states would do the trick.

### Verdict

Golem is still in developer preview and not yet ready for production, but it still offers a workable proof of concept that lets us test their WASM serverless offerings. Despite our developers having many questions, we all agreed it is a very exciting technology that may be a potential paradigm shift in cloud computing.

For us, the durable computing concept is a tempting feature. We usually deploy serverless functions as intermediaries between front ends and backend micro services, or as simple stream processors, scheduled jobs, and such. Each time, we need to implement caches and semi-transient stores for failure tolerance; with Golem, that extra infrastructure and complexity would go away. Whilst the concept cannot replace event source architectures for now, it's a very promising companion, especially for "backends for front ends."

Hivemind is very excited to see what's next for Golem!

#### References

- [https://www.golem.cloud/platform](https://www.golem.cloud/platform)
- [https://web.dev/what-is-webassembly/#webassembly](https://web.dev/what-is-webassembly/#webassembly)
- [https://www.wasm.builders/thomastaylor312/why-webassembly-belongs-outside-the-browser-331a](https://www.wasm.builders/thomastaylor312/why-webassembly-belongs-outside-the-browser-331a)
