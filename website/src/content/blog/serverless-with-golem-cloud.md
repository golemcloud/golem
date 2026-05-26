---
title: "Exploring Serverless Architecture with Golem Cloud"
date: "2023-12-04"
# date sourced from site-deploy timestamp "Mon Dec 04 2023" embedded in first wayback snapshot of post (web.archive.org/web/20231207*/https://www.golem.cloud/post/serverless-with-golem-cloud)
author: "John A. De Goes"
tags: ["Serverless", "Cloud Computing", "Durable Computing", "Architecture", "Golem Cloud"]
slug: "serverless-with-golem-cloud"
originalUrl: "https://golem.cloud/post/serverless-with-golem-cloud"
---

Serverless computing promises a fantastical world of NoOps, in which developers focus only on business logic, and leave all operational and infrastructure concerns to their cloud provider of choice.

Despite the legitimate concerns that some developers have about the current immaturity of serverless (even some [Amazon engineers](https://www.thestack.technology/amazon-prime-video-microservices-monolith/)!), this forward-looking approach to building cloud apps is here to stay.

Serverless computing is a $9B market, growing 20% each year. Most companies have at least *experimented* with AWS Lambda or similar platforms in the pursuit of a better way to build cloud apps.

What's driving the rise of serverless is that companies do not want to manage infrastructure or even think about ops. They want developers of all skill levels and backgrounds to be empowered to quickly and reliably create extraordinary value for customers.

With [Golem Cloud](https://golem.cloud), we are trying to push serverless to the next level through [durable computing](/blog/what-is-durable-computing), which gives you a robust, fault-proof foundation. This makes it possible to solve whole new classes of problems that companies would not have used serverless to solve before.

Yet, with this new way of building cloud applications comes new challenges for the many developers who are now accustomed to the powerful but complex way of building modern cloud applications.

Naive code that couldn't possibly work now becomes robust and preferred. Former best practices like storing all important data in RDBMS is now deprecated in some cases due to unnecessary complexity without benefit.

To master serverless architecture on Golem Cloud, you must follow Yoda's sage advice: "*You must unlearn what you have learned.*"

In this post, I will give you an overview of serverless computing, highlighting what Golem Cloud is bringing to the table, and walk through several examples of how we can architecture cloud apps for Golem Cloud.

### Basic Definitions

*Server-based architecture* revolves around the server, which is essentially a glorified `while` loop, which loops endlessly, accepting requests and dispatching them to request handlers, which process incoming requests and produce outgoing responses.

There are database servers, web servers, file servers, chat servers, and dozens of other types of servers. Although the protocols and the specifics vary, all of them follow the pattern I just outlined.

*Serverless architecture*, on the other hand, dispenses with the `while` loop. In its place is something called a *lambda* (AWS Lambda), *function* (Azure), or *worker* (Golem, Fastly, etc.).

Ultimately, building cloud apps does indeed require *servers*. So really the only distinction between server-based and serverless computing is *who is responsible* for the servers:

- In server-based architectures, **you are responsible** for developing, deploying, and operating the servers, including monitoring, logging, upgrades, scaling, securing, protecting and much more.
- In serverless architectures, the **cloud provider is responsible** for servers. You supply different types of workers, which are created and executed to satisfy requests that are accepted and processed by the provider's servers.

The differences are more than just cosmetic: by handling the heavy burden of maintaining, configuring, and operating complex servers, serverless computing platforms are able to offer a range of compelling benefits that seduce more and more companies every year.

### Attractive Benefits

Companies working with serverless computing no longer have to worry about a wide range of important concerns:

- **Elasticity**. As the cloud provider scales up and down servers based on demand, companies can have confidence their applications will handle traffic without having to waste money on reserved (but underutilized) compute, storage, and network.
- **Observability**. Since the cloud provider has full control over the server, they can enable a wide range of features out-of-the-box, including detailed request logging, metrics, error reporting, and alerts and notifications, without any coding or configuration.
- **Productivity**. Without DevOps, developers can focus purely on solving problems that the business has, rather than creating, configuring, and maintaining infrastructure that's common to all cloud applications, inside all companies.
- **Reliability**. Although most experienced ops teams can also achieve high reliability with sufficient resources and coverage, cloud providers live and die by their ability to provide reliable service, which leads to strongly aligned incentives.
- **Accessibility**. The skillset required to build best-practice cloud native applications is very rare, because it demands years of experience and detailed knowledge of many technologies. Serverless platforms allow even junior developers to deploy first-class cloud applications.

These benefits are significant and help explain the rise of serverless computing. That said, serverless is still evaluated and (correctly) rejected for a variety of reasons, including cost, latency, and expressiveness.

In essence, homegrown and highly-specialized server-based architectures (including monoliths) can still offer combinations of features not yet matched by mainstream serverless platforms.

But the arrow is clearly pointing to serverless as being a significant force in the future of cloud computing, with weaknesses slowly being addressed through a variety of platform improvements.

Durable computing platforms like [Golem Cloud](https://golem.cloud) increase the expressive power of classic serverless computing even further, a topic that I will discuss in the next section.

### Better Serverless

Serverless solutions like the highly-regarded *AWS Lambda* are known to suffer from various drawbacks that render it less suitable for a range of use cases. Three of the largest recognized drawbacks include:

1. **Higher-latency**. When a serverless function has not been invoked for some period of time, it is unloaded to save computing resources. When first subsequently invoked, there may be a latency spike as the function is reloaded. The more functions required to build a cloud application (typically thousands or more for a complex app), the more frequently these latency spikes will affect the application.
2. **Resource Caps**. Serverless functions are intended to complete execution quickly. Indeed, functions are generally forcibly terminated if they do not complete execution in some pre-determined amount of time. Moreover, there may be severe CPU and memory limitations. All of these resource caps, but especially the time limit, decrease potential applications for serverless technology.
3. **Statelessness**. Serverless functions may not be stateful. Most cloud applications are stateful, which must be handled in serverless architectures by pushing state into a persistent queuing service, whose individual messages are processed by families of serverless functions, leading to a complex and vendor-specific topology.

Beyond these restrictions in classic serverless computing, there is the additional challenge inherent in almost all cloud computing platforms today: they are not durable, which means that failure events such as hardware failures, OS or application updates, configuration changes, network failures, and so on, can interrupt executing code.

In practice, the fragility of modern cloud computing means that code which has transactional requirements across multiple systems, long-running code, and mission-critical code cannot be implemented naively, but must adopt sophisticated architectures for distributed systems, including event-sourcing with finite state machines and equivalent fault-tolerant architectures for distributed stateful computation.

As with any durable computing platform, [Golem Cloud](https://golem.cloud) deftly solves the fragility problem, providing a fault-tolerant foundation from which to build reliable applications (including microservice orchestration), workflows, process automations, and much more.

But as a serverless cloud computing platform, [Golem Cloud](https://golem.cloud) also targets new use cases for serverless, by removing some key restrictions:

- **Latency**. Golem is focused on low latency serverless, and while there is more work to be done here, already the choice of WASM and caching have a major impact on reducing latency. In addition, Golem's concept of a worker template is actually far more coarse-grained than a function, so a cloud application needs fewer of them than functions, reducing the likelihood of cold starts.
- **Resources**. Workers on Golem Cloud execute indefinitely, with flexible CPU and memory caps. There are no restrictions placed on how long workers may live, and indeed every aspect of the platform is built with the understanding that some workers will live "forever" (at least, as long as the business requires them to live).
- **Statefulness**. Workers are free to be stateful (for example, storing information in memory without any restriction), and with the strong guarantees that durable computing provides, this state will survive all failure  events. This lets you solve a dazzling number of problems in distributed computing, in some cases without even needing a separate database (even for transactional data).

With these sorts of features, Golem Cloud lets you deploy serverless solutions in many scenarios that were not feasible. Moreover, the resulting architecture can be far simpler than it would be using other approaches, thanks to the simplifying effects of durable computing.

Up until now, I have [introduced](/blog/what-is-durable-computing) the terms *functions*, *lambdas*, and *workers* as more or less equivalent. But deeply understanding serverless architecture on Golem Cloud will require you to understand precisely the way in which a Golem *worker* is different than a serverless *lambda*.

### Lambdas vs Workers

A lambda or function in a serverless cloud computing platform is very similar to a stateless function in any programming language.

The function accepts some input, and does something with the input (perhaps generating a response, in the case of request handlers), potentially talking to various APIs, microservices, and databases along the way.

The function is triggered by an activity managed by the cloud provider, which could be:

- **Requests**. A request that is made against a given endpoint.
- **Queues**. An item of work is added to a queue.
- **Schedules**. A specified time on a schedule is reached.
- **Events**. A lifecycle event such as an S3 upload.

Once triggered, the function executes with its input, which depends on the type of trigger, and continues until completion, or until timeout, whichever occurs first.

During execution, lambdas are *fragile*, in the sense that because they are not executed durably, there is no guarantee that once started, they will execute to completion. Failure events such as hardware failures, loss of network connectivity, OS updates, and many other scenarios can cause execution of lambdas to suddenly fail midway through, requiring complex and costly mitigation strategies to deal with inconsistencies across disparate systems.

If we were to summarize the main characteristics of lambdas, they would be as follows:

- **Fragile**. The cloud platform provides no guarantees about whether or not the code of a lambda will execute to completion. Rather, failure events will cause aborted partial execution.
- **Short-lived**. Lambdas must only be used for short-lived actions, such as straightforward processing of a request.
- **Imperative**. Lambdas are an imperative, sequential series of instructions that execute on a trigger until they are done and return some kind of trigger-specific value.
- **Stateless**. Lambdas do not have any state associated with them, nor may they do any kind of stateful computation.

Golem Cloud *workers* are natural generalizations of lambdas.

Workers are created from *worker templates*, which define the structure and logic of a worker, together with its public interface. The public interface, which is a collection of exported functions, defines the types of commands or queries that workers created from that template can process. You interact with workers through this public interface, and all interaction with a given worker is synchronized using a FIFO queue, so there is no need for worker templates to be concurrent (single-threaded Javascript is fine!).

Worker templates are conceptually similar to *classes* in object-oriented programming. Classes define the structure and logic of (a category of) objects, together with the public interface of those objects. Just like classes are used to create objects, worker templates are used to create workers, a process called *instantiation*.

A worker is a bundle of state together with worker logic. If you call one of the exported functions on a worker, then those functions may operate on the state of the worker.

Once you invoke a function on a worker, then the invocation will run until completion. However, after the invocation is complete, you may invoke the same or another function on the same worker.

Every worker on Golem Cloud is uniquely identified (within the template from which it is created) by a *worker id*. The worker id can be thought of as the "address" of a given worker. Since workers can live as long as you need them to, the concept of a worker identity is critical, since it allows sending known workers queries and commands. For typical one-worker-per-request scenarios, the worker id could be a UUID, and could be thought of as a unique identity for the request that is being processed by the worker.

Because workers on Golem Cloud are powered by durable computing, their state is durable, and can be used to reliably store any information that needs to last for the life of the worker. Moreover, the platform guarantees that if a function on a worker begins executing, then even if there is a failure event, the worker will continue executing at the exact point it left off, after a recovery process that restores the worker state.

This foundation of durable computing dramatically simplifies many use cases, ranging from microservice orchestration to workflows to process automation, and generally reduces (but does not necessarily eliminate) the need for databases, caches, persistent immutable queues, and key/value stores when building a cloud application.

If we were to summarize the main characteristics of Golem workers, they would be as follows:

- **Invincible**. Thanks to durable computing, the execution of workers and their state survives any kind of infrastructure failure event, whether that is a hardware failure, loss of connectivity, OS update, application update, and so on. From a developer's point of view, workers run on invincible servers that never go down.
- **Immortal**. There is no restriction on how long workers may live. They could live just a few milliseconds, but some types of workers may end up living for days, weeks, months, years, or even decades.
- **Reactive**. Workers don't execute an imperative, sequential series of steps. Rather, they expose a public interface (commands and queries). Other workers and the outside world can interact with a worker, asking it for information or giving it instructions.
- **Stateful**. Workers are stateful. They can store any kind of information in memory, and it is durable: it will survive for the life of the worker, whether that is milliseconds or years.

Workers are a **strict generalization** of lambdas: this means that anything you can use a lambda for, you can use a worker for. Indeed, a lambda is like a worker that doesn't need any state, and which exposes a single function that takes some input and produces some output.

This implies that classic serverless architectures work just fine on Golem Cloud, and still give you the benefits of durable execution (the guarantee that your function will be fully executed, regardless of any infrastructure-level failure events).

However, it also opens up the door to new kinds of architectures, and many more use cases, with far fewer triggers and use of third-party cloud services such as queues.

Now that you understand the differences between lambdas and workers, it's time for us to look at several different examples of serverless architectures on Golem Cloud.

We'll begin with an e-commerce checkout example.

### Checkout

In this example, let's assume we are building an e-commerce application, and the part of the application we're concerned about now is the part that does checkout for a user.

Note that there are many ways to build the whole application on Golem Cloud. But let's assume we have already built that and just want to leverage Golem Cloud for the checkout process itself.

Checkout of customers is initiated from both mobile and web applications through a REST API endpoint, let's say `/checkout`. This endpoint is passed the full contents of the shopping cart, together with pre-validated payment and shipping details.

As part of the checkout process, we need to complete the following steps:

1. Reserve inventory for the order by calling an API. If this fails, then we have to split the order into two orders, so we can dispatch the part of the order that can be fulfilled right away.
2. Charge the payment method. If this fails, the checkout process itself will fail, which must revert the reservation of inventory. Otherwise, the process can continue.
3. Create a shipment in the shipping provider so we can obtain a tracking number.
4. Dispatch the order to the warehouse for fulfillment.
5. Finally, send a confirmation process to the customer, including the tracking number.

The challenge of this checkout process is not *essential* complexity: after all, we can describe the steps simply and in just a few sentences. Rather, the challenge in architecting checkout is the inherent fragility of modern cloud computing.

If there is an infrastructure failure event after some of the steps, then the distributed state of the entire system is inconsistent: for example, inventory may be reserved and the customer charged, but the user will never get an order confirmation, and the order will never be dispatched.

These problems are not just inconvenient, they are costly and unacceptable for most businesses, so as cloud engineers, we have to find a way to satisfy the business requirements despite the limitations of the underlying technologies we are developing on.

Classically, we would solve this lack of transactionality by using event sourcing: by storing events persistently (using Apache Kafka, for example), and viewing the state of the checkout process as being an aggregate over all events related to the checkout, we are able to implement recovery processes that can withstand failure events.

When designing for Golem Cloud, however, we can adopt a radically simpler architecture:

1. For each request to the `/checkout` endpoint, configure the API Gateway to create a new worker from a checkout worker template, and invoke a *checkout function* on the worker.
2. The worker's checkout function completes all steps in order, appropriately handling rollback in the event of failure using error recovery mechanisms in the host programming language (for example, try / catch in Javascript).
3. When done, the worker ends life, surviving only as long as it takes to complete the checkout process.

This solution is *impossibly* simple: it can't work without durable computing. Yet, with durable computing, the impossibly simple solution becomes the robust, preferred way to implement checkout.

Golem Cloud gives you what is, in effect, an invincible virtual server to process each request on. This lets you radically simplify the way you develop even small parts of your overall cloud app.

### Shopping Cart

In this example, let's assume we are building a shopping cart for an e-commerce application. As in the previous example, we will assume the application already exists and that we just want to redo the shopping cart using Golem Cloud to improve user experience.

Most e-commerce sites need two different types of shopping carts:

1. A shopping cart for a user who is not logged in. This allows users to add items to the shopping cart before they have logged in or created an account with the store. The shopping cart is tied to the device the user utilizes to interact with the storefront. So if the user switches to another device, they will lose the contents of their shopping cart.
2. A shopping cart for a user who is logged in. Because the identity of the user is known, if they switch to another device before checkout, the contents of their shopping cart are preserved, and they can continue shopping from the new device without interruption.

If a user logs in after adding items to their (anonymous) shopping cart, then the contents of that shopping cart are merged into their persistent shopping cart, so the user does not lose any of the items they added to either shopping cart.

Classically, we would solve the shopping cart problem by using a highly scalable key/value store. We would then deploy a stateless microservice behind an API Gateway (or perhaps, just a REST API service) that is responsible for maintaining the state of the shopping cart.

The key/value store gives us the ability to persist the shopping cart robustly, but there may be some danger of clobbering shopping cart entries in the presence of concurrency, depending on the technology that we choose.

In addition, the logic of the shopping cart service will be cluttered with lots of boilerplate that serializes and deserializes the items and the shopping cart as a whole into and out of storage.

In Golem Cloud, we can take advantage of durable state to create a much simpler and more robust architecture:

1. We create one worker template to store and manage the shopping cart for a single user. This template exports functions to add and remove items from the shopping cart. The shopping cart is an in-memory map from product id to the number of items the user wishes to buy.
2. We use the API Gateway to map the root endpoint `/users/{user-id}/shopping-cart/*` to functions on a worker whose identity is equal to the user id. The API Gateway will create that worker if necessary, or will simply invoke functions on the worker if it already exists.
3. We use the API Gateway to map the root endpoint `/anon/{device-id}/shopping-cart/*` to functions on a worker whose identity is equal to the device id. As with the other API, the API Gateway will create the worker if necessary, or just invoke functions on the worker if it already exists.

As with the preceding example, implementing a shopping cart using Golem Cloud is impossibly simple: the naive thing of just storing raw data structures inside a map turns out to be the best possible solution!

In this solution, the shopping carts are robust, impervious to infrastructure failures, and changes to them are atomic and fine-grained (unlike with some key/value stores, which are subject to clobbering in the presence of concurrent updates with the same key).

The fact that workers have durable state can be effectively used by cloud engineers to partition very large states (in this case, the state of all shopping carts across all users) into much smaller pieces, giving us the ability to store them in-memory and operate transactionally on each piece via worker functions.

### Auction

In this example, let's assume we are building an auction site, similar to Ebay, where we have perhaps millions of auctions, each with its own bidders and settlement process.

There are many areas we could focus on to discuss serverless architecture, and for each area, there are multiple ways to architect a solution.

For now, let's focus on both the logic and data of each auction, and how we could implement that solely using Golem Cloud, without any databases.

Classically, we would probably use a sharded RDBMS to maintain the state of all auctions, because we need transactional guarantees on bidders, bids, and winning bids. Then we would stand up stateless services that provide the auctioning logic and update the database.

With Golem Cloud, however, we can adopt a much simpler architecture:

1. We create one worker template to store and manage the state of each auction. The worker template defines functions for adding new bids, keeping track of bidders, and closing the auction, including settlement.
2. We use the API Gateway to map the root endpoint `/auctions/{auction-id}/*` to various functions on a worker whose id is equal to the auction id. Mobile and web clients interact directly with this API to interact with change and manipulate each auction.

This is a very beautiful example of the fact that in many problems, there is a way to shard total state in such a way that each worker can achieve the transactionality and performance guarantees necessary to make a database-free solution viable.

Although you don't need to take advantage of durable state in your own Golem Cloud applications (indeed, transactional execution of code is a sufficient win), it holds tremendous promise for simplifying many types of cloud applications.

### Summary

The name *serverless* may not survive forever, but the *invention* of serverless computing, in which we push our application logic into the cloud, and rely on the cloud to handle all the messy details, is definitely here to stay, and will require you to change how you architect cloud applications.

Serverless computing gives us elasticity, observability, productivity, and accessibility, providing a compelling package for modern businesses who want to drive value rather than maintain infrastructure.

Although there are still challenges with serverless computing, I personally believe that serverless will rise to meet these challenges.

Golem Cloud, a cloud computing platform built on durable computing, gives serverless the newfound ability to handle low-latency, long-running, stateful use cases. These use cases include microservice orchestration, workflows, process automation, and, thanks to durable state, countless problems in distributed computation.

In order to achieve this, Golem Cloud generalizes a function (stateless, fragile, short-lived, and one-shot) into a worker (stateful, durable, immortal, and reactive). Golem workers can do everything that lambdas can do, but they can do so much more.

Architecting serverless applications is quite different than architecting server-based applications. Although Golem Cloud lets you use established serverless architectures, you have more flexibility because your workers are invincible. You can leverage this invincibility merely for transactionality (like in microservice orchestration and workflows), or you can also leverage it for storage, using workers to partition distributed data in your cloud application.

Hopefully you found this introduction to serverless architecture on Golem Cloud useful. Please share with us how you are architecting your applications on Golem Cloud!
