---
title: "Unveiling Golem Cloud"
date: "2023-08-01"
# date sourced from site-deploy timestamp "Tue Aug 01 2023" embedded in first wayback snapshot of post (web.archive.org/web/20230801134445/https://www.golem.cloud/post/unveiling-golem-cloud); earliest wayback snapshot of the golem.cloud site overall
author: "Golem Cloud"
slug: "unveiling-golem-cloud"
originalUrl: "https://golem.cloud/post/unveiling-golem-cloud"
---

How do we cloud developers build robust, reliable and scalable software? The tech industry has spent unimaginable resources creating ever more complex answers to this question.

Today, we are rethinking the very foundations of cloud computing, and unveiling a radical and simple alternative.

One that dissolves the layers of complexity that have accreted over two decades of programming in the cloud era.

This solution is called Golem Cloud, our flagship product, which we are launching today in Developer Preview.

## Golem Cloud

Golem Cloud is a quantum leap in the evolution of cloud computing. Golem Cloud enables you to deploy immortal, reactive workers that are executed invincibly:

- Immortal. Although latency of Golem Cloud workers is extremely low, workers do not have to execute within milliseconds (although of course that's possible). They can live for days, weeks, months, or even years. As long as the business requires them to live.
- Reactive. Golem Cloud workers may react to external queries and commands, rather than just executing a fixed sequence of operations in response to a trigger. They can also spawn new workers, with bidirectional communication.
- Invincible. Golem Cloud workers execute using the new paradigm of durable computing, which allows them to survive hardware failures, updates, and upgrades, and lets them obtain robust persistence guarantees with ordinary memory.

These advances are not achieved by forcing you to use a particular programming language, framework, software-development kit, or library. Rather, they are achieved through WebAssembly.

WebAssembly is a new compilation target for a growing number of programming languages. Combined with the WASI standard, WebAssembly provides a specification for a secure, capability-oriented virtual machine. A specification so constrained that it's possible to provide new ways to execute WebAssembly programs.

Golem Cloud provides a runtime for WebAssembly programs that, without introducing any proprietary extensions, frameworks, or SDKs, executes WebAssembly programs in a reactive, invincible, and immortal way, providing a powerful new foundation for cloud applications.

### A New Paradigm

The core idea of Golem is simple: take ordinary programs, written in any programming language that supports WebAssembly, and execute them in a way that is resilient to hardware failures, upgrades, and updates.

Yet the idea of invincible programs is startling in its ramifications.

Today, if we wanted to write code that clears a shopping cart, waits 24 hours, and then sends the user a list of similar items, then we would not write it in the following straightforward way:

```rust
fn remove_from_cart(user: User, cart: ShoppingCart, pid: u32) -> Result<(), Err> {
    cart.remove_all();

    sleep(Duration::from_secs(24 * 60 * 60));

    let product = get_product(pid)?;

    let similar_products = get_similar_products(pid)?;

    send_similar_products_email(user.email, similar_products)?;

    Ok(())
}
```

Why? Because cloud infrastructure is unreliable, so our program can fail at any time. So while we might succeed in removing an item from the cart, we have no guarantee that the email will be sent to the user.

The server could go down or be restarted many times for various reasons, including important updates to the application, configuration, operating system, development stack, and much more.

Similarly, if we want to write code to process an order for a company like Amazon, we would not write it as follows:

```rust
pub fn process_order(order: Order, user: User, payment_method: PaymentMethod) ->
        Result<Order, &'static str> {
    let product = get_product(order.product_id)?;

    if product.stock < order.quantity {
        return Err("Insufficient inventory");
    }

    let reservation =
        update_inventory(product.id, order.quantity)?;

    let payment_intent =
        create_payment_intent(user.id,
            order.total_amount, payment_method)?;

    let payment = charge_payment(payment_intent)?;

    if payment.status != "succeeded" {
        update_inventory(product.id, -order.quantity)?;
        return Err("Payment failed");
    }

    let fulfillment_order = dispatch_order(order, user)?;

    send_order_confirmation(user.email, fulfillment_order)?;

    Ok(order)
}
```

The reason we can't write straightforward code like this is because our program can fail at any point in time. For example, we have no guarantee that, after we successfully charge the user's credit card, we will continue to dispatch the order to the warehouse for fulfillment.

If our program fails in the wrong place, then the results could be catastrophic for customers, and also for the business!

The unreliable nature of cloud computing forces us to adopt heavyweight tools like event-sourcing in order to reliably orchestrate multi-step processes, or to execute longer processes over periods of time and in response to third-party agents and systems.

These heavyweight tools demand more experience and skill from developers, far more code, complex architectures, and ops. Moreover, while they solve the problem, they do so at high cost, slowing down the pace at which businesses can innovate.

The magic of Golem Cloud is that we can write simple, straightforward programs that clearly reflect underlying business logic, and they will run to completion, regardless of failure events.

When there are failures, or particular nodes get overloaded, Golem Cloud relocates workers, including their state, to new machines, where they resume execution at the exact point they left off.

### Beyond Workflows

Golem Cloud may sound like a platform for deploying workflows. Indeed, workflow platforms like [Temporal.io](https://temporal.io) are designed to execute workflows reliably, from start to finish, regardless of failure events.

Yet, Golem Cloud goes far beyond just workflows.

Thanks to the component model, WebAssembly allows us to "export" functions from our program (in a language-specific way). Golem Cloud detects these exported functions and automatically makes them available via a secured HTTP API.

Workers deployed on Golem Cloud are reactive. Any function you export from your code can be called by the outside world or by other workers. In this way, workers can respond to commands and queries, rather than just blindly executing steps in a predetermined sequence.

Once you have launched a worker, you can call functions that the worker exports, either to query the internal state of the worker, or instruct it to perform some action. Workers, in turn, can spawn new workers, and communicate with them bidirectionally through the automatic API that Golem creates for them.

Thanks to durable computing, all of the worker's state is durable, just like a database, which means that some applications don't even need to use a separate database.

For example, you could assign one worker per user to keep track of their shopping cart. An in-memory array is more than enough to model the shopping cart, since it's every bit as durable as storing the cart in a database.

Golem Cloud is thus strictly more powerful than workflow solutions. Although we are still in the process of determining just how expressive Golem Cloud workers are, they are at least as powerful as distributed, persistent actor frameworks, such as Akka and Dapr.

Golem Cloud subsumes both workflow programming and distributed actor programming in a new, low-latency paradigm powered by durable computing. It's a quantum leap over today's cloud computing solutions.

### Strange New Worlds

Durable computing will surely lead to new ways of solving problems that we can now scarcely imagine.

As one example, Golem Cloud could bring back server-side web frameworks. In a world of durable computing, one worker could serve an app for a single user, storing all user data in memory.

Aside from offering a vastly simpler programming paradigm, one advantage of server-side computing is that instead of downloading 2.5 MB worth of Javascript before the user sees anything (the average size these days!), the user can begin interacting with a program immediately, with all rendering and logic happening server-side.

The state of the user's application is persisted in memory, without manual encoding and decoding from NoSQL or relational databases, and the worker running the program could be automatically migrated to servers close to the user's current location. Perhaps even to the device the user is working on, if capabilities permit!

As we all explore exciting new applications of durable computing, we encourage everyone to post about new frameworks, libraries, and architectural patterns that may emerge. It's an exciting time!

### Developer Preview

As of today, Golem Cloud is officially live in Developer Preview. You can immediately sign up for an account, and begin deploying immortal, reactive, invincible workers on Golem Cloud.

During Developer Preview, Golem Cloud is completely free. Keep in mind, however, that we are currently not providing specific guarantees on worker lifespan, reliability, or backward compatibility. So we encourage you to build amazing apps on Golem Cloud, just don't put anything into production yet!

For the next 6 months (give or take), we will be hard at work listening and responding to feedback, and implementing the right set of features for the commercial launch. In particular, security, stability, reliability, observability, and a full suite of management and diagnostic tools will be a key focus of the commercial launch.

We will launch Golem Commercial with a free Developer tier that lets you deploy low-volume applications. Through efficient infrastructure, we will be able to offer a well-thought out pricing model that lets you build as much of your application as you like on Golem Cloud.

### The Future Is Here

Building modern cloud applications is complex, especially those that require reliability, durable state, reactivity, and long lifespans.

Golem Cloud is a quantum leap in the evolution of cloud infrastructure. Unlike classic cloud-native development, Golem Cloud is simple, and lets you focus on business logic and not infrastructure. Unlike ordinary serverless workers (like AWS Lambda), workers deployed to Golem Cloud are reactive, invincible, and immortal.

This intoxicating balance of power and simplicity lets you build solutions for countless challenging problems, but without the complexity of cloud-native applications.

You can build cloud applications confidently, with straightforward code that reliably runs to completion. With in-memory state that is as durable as a database. With clean and clear business logic.

We still have more work ahead of us as we extend, improve, and polish Golem Cloud, and WebAssembly itself has some maturing to do. But after seeing early fruits, the whole team here is very excited for what we believe will be a complete re-invention of cloud computing

The future is here... are you ready for it?
