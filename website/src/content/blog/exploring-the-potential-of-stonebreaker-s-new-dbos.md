---
title: "Exploring the Potential of Stonebraker's New DBOS"
author: "John A. De Goes"
slug: "exploring-the-potential-of-stonebreaker-s-new-dbos"
originalUrl: "https://golem.cloud/post/exploring-the-potential-of-stonebreaker-s-new-dbos"
date: "2024-03-14"
# date sourced from site-deploy timestamp "Thu Mar 14 2024" embedded in first wayback snapshot of post (web.archive.org/web/20240407005815/https://www.golem.cloud/post/exploring-the-potential-of-stonebreaker-s-new-dbos); post absent from blog snapshot 2024-03-10 (deploy Feb 19 2024)
---

Michael Stonebraker is a legend in the field of database technology—his influence forever shaping how data is managed and analyzed.

Stonebraker spearheaded the development of Ingres and Postgres, introducing key concepts that underpin modern databases. His later ventures, including Vertica, VoltDB, and SciDB, pushed the boundaries of data warehousing, real-time data, and complex data.

Given his pivotal position in the history of database technology, when Stonebraker recently introduced DBOS, a new cloud infrastructure company, the world has rightly taken notice.

## Overview

Briefly, the core of DBOS consists of an “operating system” whose state is kept in a fast and scalable relational database, which forms the backbone of the system, providing discoverability and transactionality as fundamental aspects.

Yet, the company seems to acknowledge the reality that the market for new “operating systems” is minimal, especially if those new operating systems require rethinking every aspect of application design, and reinventing all of the libraries, frameworks, and applications built on existing operating systems.

So rather than sell or promote an "operating system", per se, the company is instead selling a so-called *transactional serverless platform*, which consists of the following components:

- A cloud-hosted implementation of DBOS, termed *DBOS Cloud*.
- A Typescript SDK, which is used to build apps that can be deployed on DBOS Cloud.
- A testing runtime, which can be used to execute your apps locally for purposes of testing.

Interestingly, the phrase “transactional serverless” does not refer to an existing technology category, raising the question of what category of problems this offering is intended to solve.

## What the Heck Is Transactional Serverless?

To better understand why DBOS is using the phrase *transactional serverless* to refer to their new offering, it’s helpful to rewind the clock by a year, to April 14, 2023.

In a bold if controversial post, a trio of authors at the prestigious venture capital firm Andreessen Horowitz came out with an article entitled, [The Modern Transactional Stack](https://a16z.com/the-modern-transactional-stack/).

In this article, A16Z argued that there was a hidden pain in modern cloud app development.

To understand the pain deeply, however, it’s necessary to understand the high value of transactions, as well as the modern decentralization of application state.

Let’s look at each in the following sections.

## The High Value of Transactions

Transactions are the solution to the problem of how to update state in a reliable way.

In a database transaction, lots of disparate updates are specified and executed as a single atomic unit, called a *transaction*.

The database then gives us the following strong guarantees (ACID):

- **Atomicity**. The database ensures that our updates either happen fully, or not at all.
- **Consistency**. The database ensures that our state proceeds from one consistent state to the next.
- **Isolation**. The database ensures that concurrent updates happen as if they were sequential, to isolate in-progress transactions from each other.
- **Durability**. If a transaction is successful, the updates will survive faults, such as system failure, because they have been made durable.

These ACID guarantees give us the gold standard of high reliability, in a way that completely isolates application developers from the implementation details.

With transactions, developers can focus on what state they want to update and in what unit (the *what*), and the database itself can handle the difficult challenges of ensuring those updates happen reliably (the *how*).

## The Decentralization of Application State

In the old mainframe days, the state of most systems was stored in a single relational database. In the cloud era, however, the state of a system is no longer stored in a single database.

Rather, state for modern applications is distributed across many databases, third-party APIs, message brokers, and much more, often replicated in many different forms.

Whereas the state of mainframe applications was mostly updated using transactional SQL, the state of modern distributed systems is mostly updated with non-transactional application logic.

There’s a big distinction between transactional SQL and non-transactional code, which is the key to unlocking the “hidden pain” that A16Z alluded to more than a year ago.

## The Hidden Pain of Modern Cloud App Development

Legacy applications update most of their state with transactional SQL, and are therefore reliable by default, without application developers having to think about it.

Modern cloud applications, on the other hand, update most of their state with non-transactional application logic, and therefore, they are unreliable by default.

The hidden pain of modern cloud app development is that a significant part of the cost of developing, maintaining, and operating applications is compensating for unreliability, caused by our shift from centralized state to distributed state.

Many patterns we reach for by default these days, including event-sourcing, state machines, and CQRS, exist as compensatory mechanisms for the unreliability of application logic.

With this background, we can now more easily understand why DBOS coined the phrase *transactional serverless* to describe their inaugural offering.

## Transactional Serverless Defined

A *transactional serverless platform* provides a way to expand the scope of transactions from inside a single relational database (*transactional*), to across arbitrary application logic inside request handlers (*serverless*), with the intent to give developers a much simpler way to develop high-reliability applications.

Stated less precisely: DBOS brings transactions to modern cloud apps, which use application logic to update decentralized state, so you can easily attain the reliability of legacy apps, which use SQL to update centralized state.

This promise sounds too good to be true: how can we get transactions on arbitrary application logic, given that transactions provide us with features like rollback, which aren’t even supported by many of the third-party systems that our cloud apps interact with?

Let’s find out if the reality of DBOS lives up to its hype!

## Overview of DBOS

DBOS being a serverless offering, the primary building block that it provides you with to build your backend applications is the *function*.

Using Typescript decorators, functions can be associated with routes, as in the following *Hello World* example:

```typescript
import { HandlerContext, GetApi } from '@dbos-inc/dbos-sdk'
export class Greetings {
 @GetApi('/greeting/:friend')
 static async Greeting(ctxt: HandlerContext, friend: string) {
   return `Greetings, ${friend}!`;
 }
}
```

When you deploy your Typescript application, DBOS creates a web server that delegates the handling of different routes to the appropriately-decorated functions in your application.

Interestingly, if you create routes and you implement them with ordinary Typescript code, then your application logic is not transactional, or even close to transactional.

Rather, a straightforward implementation of serverless functions will not provide your application with any benefits over any other serverless platform.

To tap into DBOS-specific functionality, you have to learn, understand, and use the concepts of *workflows*, *communicators*, *transactions*, and *events*:

- **Workflows**. A workflow is a Typescript function whose body consists of a linear sequence of invocations of communicators and transactions.
- **Communicators**. A communicator is a Typescript function that invokes other APIs, microservices, and so forth.
- **Transactions**. A transaction is a Typescript function that executes some SQL queries on a relational database provided by DBOS.
- **Events**. Events are a concept with API support that allow communication from the outside world to workflows, and from one workflow to another.

While this article is not intended to be a tutorial on how to use DBOS, it's worth exploring these concepts in a few paragraphs so our exploration can be properly informed.

### Workflows

A workflow is the "workhorse" of the transactional guarantees that DBOS provides.

To denote a Typescript function as a workflow, you have to do more than use the @Workflow() decorator: you have to promise to abide by a set of contracts that DBOS does not and cannot statically check.

1. **Default Deterministic**. All code inside the workflow function, or in any function it invokes transitively, must be deterministic. That is, given the same inputs to the workflow function, it must compute the same value. Obviously, this severe limitation would lead to workflow functions that are not very useful, so an exception is made for collaborators.
2. **Controlled Communication**. Whenever a workflow wants to do something that isn't deterministic (such as invoking an API or generating a random number), it must do so through using the DBOS API to invoke a *communicator*. This extra level of indirection is necessary to achieve workflow guarantees.
3. **Controlled Transactions**. Whenever a workflow wants to interact with the application's relational database, it must do so through the DBOS API to invoke a transaction. As with communication, this level of indirection is foundational for workflow guarantees.

In exchange for being a good citizen, DBOS provides you with the following guarantees:

1. **Atomicity**. If the execution of a workflow begins, it will always complete. Even if interrupted by some system failure, DBOS will reliably resume the workflows at the point where the failure occurred.
2. **Exactly Once Database Transactions**. Regardless of system faults, DBOS will execute each database transaction once and exactly once.
3. **At Least Once Communications**. Regardless of system faults, DBOS will execute communicators *at least once*. But once known to be executed successfully in the context of a workflow, DBOS will never execute the same communicator again.

The following code snippet shows an example workflow that does all its communication and database logic using communicators and transactions:

```typescript
class Greetings {
   @Workflow()
   @GetApi("/greeting/:friend")
   static async GreetingWorkflow(ctxt: WorkflowContext, friend: string) {
       const noteContent = `Thank you for being awesome, ${friend}!`;
       await ctxt.invoke(Greetings).SendGreetingEmail(friend, noteContent);
       await ctxt.invoke(Greetings).InsertGreeting(friend, noteContent);
       ctxt.logger.info(`Greeting sent to ${friend}!`);
       return noteContent;
   }
}
```

### All The Rest

Collaborators may perform async network requests, access the file system, generate random numbers, or do anything else that is not deterministic.

In the context of a collaborator invocation, there are no transactional guarantees.

Transactions have access to the application's relational database, and may execute arbitrary SQL against this database.

Transactions do indeed have transactional guarantees: they will execute exactly once, and they will execute in a fashion that provides all ACID guarantees.

Finally, events provide a way to coordinate workflow activity with the activity of external services (allowing workflows to be used in webhooks), and with the activity of other workflows.

We're now in a position to take a closer look at what DBOS gives us.

## An Analysis of DBOS

From analyzing the architecture of DBOS, we can conclude the building blocks that DBOS provides application developers are indeed sufficient to construct high-reliability applications.

The strong guarantees of workflows remain unaffected by system failures or hardware failures, introducing only a relatively small delay in execution, as DBOS detects and responds to the failure events, and restores running workflows to where they left off.

This allows applications to reliably update state that is distributed across multiple databases, third-party APIs, message brokers, and anywhere else.

In a way, this gives us transactionality for our application logic. However, the transactionality that DBOS provides differs from database transactionality in the following key ways:

- **No Rollback**. Although DBOS can rollback database transactions, DBOS cannot rollback communicators. As a result, if a workflow intentionally fails midway through its execution, perhaps because of a communicator failure, then the partial work done by the workflow will not be rolled back automatically.
- **At-Least Once**. The logic directly inside a workflow is executed *effectively once*, assuming developers are following the contract. Moreover, the updates inside transactions are executed *exactly once*. However, communicators may be invoked multiple times in the context of the same workflow, thus offering only *at least once* execution semantics.

These limitations are not really limitations of DBOS, per se, but rather limitations necessitated by the arbitrary nature of communicators. Communicators can literally do anything, including calling APIs that were never designed to offer idempotency or rollback.

On the one hand, the open-ended nature of communicators allows using existing web services, APIs, microservices, and the like, without any customizations. On the other hand, it does imply that no system as expressive as this can provide both rollback and workflow-wide, exactly-once execution semantics.

Fortunately, in practice, rollback semantics can be implemented by developers using the Saga pattern, which allows them to encode their unique knowledge of how different communicators might be rolled back, in the event of a non-recoverable failure inside a workflow.

Similarly, *exactly-once* semantics can often be built atop *at-least-once* semantics.

For example, instead of calling an API that charges a user for a purchase, you can first call another API to create a purchase intent, and then call the first API to execute the purchase intent. Such "tricks", which are effectively powered by *idempotency keys* (a feature natively supported by DBOS), allow developers to inject *exactly-once* semantics where necessary.

Because both of the transactional weaknesses are a natural and necessary consequence, given the level of expressivity supported by DBOS, and because they can be compensated for in ways that are far easier than solving the high-reliability problem, they are minor.

It appears, then, that DBOS delivers on its promise of giving applications transactionality–at least, as much transactionality as you could hope for, given the unconstrained nature of application logic.

Although there is no doubt transactional application logic is valuable, there remains a very significant question to answer about the novelty of DBOS.

## Is DBOS Really Novel?

In the past couple of years, the category of solutions for so-called *durable execution* has grown tremendously, driven by the growing awareness and adoption of market-leader Temporal.

Durable execution solutions tend not to approach the reliability problem from the perspective of database transactions, and their marketing seldom (if ever) makes this comparison.

Rather, durable execution solutions come not (primarily) from *database developers* (like Stonebraker and Palmer), but from *application developers*.

Application developers have long admired the durability guarantees of databases, and struggled to achieve these with convoluted and complicated architectures. Durable execution solutions are their attempt to bring the durability of databases to their application logic.

Meanwhile, database developers have long admired the expressivity of applications, which can execute arbitrary logic, compared to the (necessarily) crippled expressive power of SQL.

DBOS is an attempt to incorporate application logic into the powerful transactional model of the database.

Although these two different camps may approach the problem in different ways, it is incorrect to consider them different solutions for different markets. In fact, they are different approaches designed to target the same market, albeit in different ways, and with different tradeoffs.

One could argue that even though DBOS is not building a new type of solution, they are doing it in a different way than durable execution providers. We'll explore this idea in a moment, but it's also worth pointing out DBOS is *not* the first attempt to bring application logic to the database.

Although historical attempts are plenty, they failed mostly because they were attempts to extract additional revenue from enterprises by bolting on hacks, resulting in unsatisfying semantics and performance, and numerous edge cases.

Yet, one modern example stands out: [Convex](https://www.convex.dev/). Convex is a Javascript database where queries are code on in-memory structures. Convex does not appear to be targeting the same space as durable execution, but the building blocks are there for a solution in the space.

So even in the category of databases that extend some type of transactional support to arbitrary application logic, DBOS is not unique.

Now let’s take a look at how DBOS compares to existing durable execution solutions.

### Comparing DBOS to Durable Async/Await

Those working in the space of durable execution will notice a strong similarity between concepts in DBOS and concepts in durable execution platforms.

For example, DBOS Workflows correspond to Temporal Workflows; DBOS Communicators correspond to Temporal Activities, and so on.

Moreover, the most recent wave of durable execution platforms seems focused on Typescript or Javascript. Recently termed *durable async/await*, these durable execution solutions include [Inngest](https://www.inngest.com/), [Restate](https://restate.dev/), [Trigger](https://trigger.dev/), [Resonate](https://www.resonatehq.io/), and [Effectful](https://www.effectful.co/).

All of these solutions allow building the same solutions one can build using DBOS, and often in almost the same way, with just minor syntactical differences.

The main and important distinction between DBOS and durable async/await is that DBOS has managed to seamlessly integrate database transactions, in a way that guarantees those database transactions will be executed exactly once as part of a workflow.

To achieve the same functionality with "raw" durable async/await, it would be necessary to write some custom code (for example, maintain an idempotency key table, and use this table to ensure no SQL query were ever executed twice).

From my point of view, however, I do believe that every existing *durable async/await* solution can incorporate *exactly-once* transactions without any significant architectural changes.

In other words, the improvement that DBOS provides over *durable async/await* is incremental, rather than revolutionary, and does not represent an existential crisis for existing solutions.

### Comparing DBOS to Transparent Durable Execution

As the founder and creator of [Golem Cloud](https://golem.cloud), a solution in the space of *transparent* durable execution, I also feel the need to comment on the relationship between DBOS and Golem (or Flawless, the only other solution in the same space).

Transparent durable execution platforms target the same problems, but they aim to do so in a way that doesn't require programmers to follow any contract or use any SDK.

Using any code, any frameworks, any libraries, and even any languages, solutions like [Golem](https://github.com/golemcloud/golem) provide fully durable execution without any custom code.

When I first read a high-level summary about DBOS, I thought DBOS might provide an implementation of Typescript that used a database for storing all program state. This would provide another entry in the space of transparent durable execution.

However, DBOS turned out to be less ambitious than I originally thought, limiting the scope of its improvements to database transactions.

These improvements are real, but incremental, and we contributors to Golem know how to bring more seamless support for database transactions to the platform, without any architectural changes or major feature additions.

## Summary

In DBOS, a new company and product from database legend Stonebraker, we find a solution to the hidden pain of modern cloud app development identified by Andreessen Horowitz.

This pain, stemming from the decentralization of application state and the reliance on non-transactional application logic, has led to systems that are unreliable by default. Solving this unreliability problem has historically required complex architectures, such as event-sourcing, which impose significant costs on development, maintenance, and operations.

DBOS aims to rectify this problem by ensuring transactional-like behavior across arbitrary application logic inside request handlers, effectively marrying the durability and reliability of database transactions with the flexibility and expressivity of application logic.

DBOS is not the first to attempt bringing transactions to application logic, but its approach is distinct enough to merit attention. However, when judged by its benefits rather than approach, the innovations of DBOS are incremental rather than revolutionary. DBOS is another entry into the space of durable execution, similar to some existing solutions, but with its own trade-offs.

For my part, I'm excited for the launch of DBOS and hope that this new entry into the market from Stonebraker and Palmer will help to legitimize the space of durable execution.
