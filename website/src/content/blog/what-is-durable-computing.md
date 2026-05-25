---
title: "What Is Durable Computing?"
date: "2023-08-08"
# date sourced from site-deploy timestamp "Tue Aug 08 2023" embedded in first wayback snapshot of post (web.archive.org/web/20230811123355/https://www.golem.cloud/post/what-is-durable-computing); post present in earliest blog index snapshot 20230811 (deploy Aug 8 2023)
author: "John A. De Goes"
slug: "what-is-durable-computing"
originalUrl: "https://golem.cloud/post/what-is-durable-computing"
---

In [unveiling Golem Cloud](/blog/unveiling-golem-cloud) last week, we launched a new cloud computing platform that can execute serverless workers **invincibly**.

Although durable computing is not a new idea, Golem Cloud is the first mainstream attempt to deliver durable computing on commodity hardware, without specialized virtualization technology, and without forcing developers to adopt a particular programming language, software development kit, or way of writing their applications.

In marketing for Golem Cloud, we describe Golem workers as **invincible**, which catches attention but begs for clarification. So in this post, I'm going to explain exactly what we mean by the word *invincible*.

### Invincible

When we say that workers deployed to Golem Cloud are *invincible*, we mean that your workers will continue to execute even if the node they are running on is downed, due to hardware failures, updates, upgrades, or connectivity issues.

How is this even possible? It sounds like advanced alien technology!

It's possible because the nodes that run your workers are continuously snapshotting the state of workers. Not just their heap memory, but also their stack memory, and including the so-called *instruction pointer*, which determines where in some procedure your code is executing at the current moment in time. A full snapshot of everything.

These continuous snapshots are not whole-memory snapshots, because that would reduce performance and increase latency. Instead, the snapshots are implemented through capturing tiny incremental deltas, which permits reconstructing the full state during recovery, while keeping performance high and latency low.

These snapshots are the key to powering *recovery* after *failure.*

### Failure & Recovery

As your worker is executing on Golem Cloud, it is possible that, midway in execution (potentially in the middle of some procedure), the node running your worker experiences a failure event.

Perhaps the hardware fails, or possibly the node is just restarted for critical updates. Possibly, there's a connectivity issue that renders the node effectively down.

After such a failure event, all workers running on the downed node will be recovered and resume execution on new nodes.

During the recovery process, the state of workers is restored from the incremental snapshots, and their execution resumes wherever it left off—at the exact point where the workers were executing at the moment of failure. This is true even when the failure occurs in the middle of some function call, after some statements completed, but before others could be executed.

Automatic recovery from failure using incremental snapshots is how durable computing on commodity platforms is possible. However, I should discuss a few caveats.

### At Least Once

Now, I mentioned that resumption of a worker occurs exactly where they were executing at the moment of failure. However, I need to be more precise about resumption semantics in the presence of host calls.

If your worker code was executing a host call, such as performing an HTTP request, and it was not known the host call returned before the failure event, then that host call will be *repeated* during recovery.

This means, for example, if your worker makes a call to process a credit card transaction, and the node is downed *during* this processing, before the response is returned and processed by your worker, then the call to process the transaction will be repeated during recovery.

This "at least once" semantics with respect to in-process host calls is something you have to design for. In the previous payment processing example, you could use Stripe's *PaymentIntent* to prevent duplicate billing (create the payment intent in one line of code, and then in the next line, perform the charge).

### Recovery Latency

*Invincibility* does not imply *instantaneous* recovery: it takes some time to detect a downed worker, assign it to a new node, and recover its state. This adds latency to all interactions involving the worker.

Besides additional latency, this recovery is not visible to the outside world. Both to your worker and to the outside world, it's as if your workers execute on an invincible machine.

Advanced alien technology, indeed!

### Invincibility?

Just because your workers (effectively) execute on an invincible machine, doesn't mean it's impossible for your workers to die.

For example, maybe you have a null pointer exception in your Java worker that you don't catch. Or maybe you panic in Rust. Maybe you do something unsafe in C/C++. The possibilities are endless, and no technology can guard against internal bugs and failures!

Invincible workers can *still* die, just not due to *infrastructure* reasons. They can die for *internal* reasons. Or they can die for *external* reasons, too, if you decide you really need to kill a worker.

Yet, the guarantee that your workers are *invincible*, in this precisely defined sense of the word, still proves enormously useful to building modern cloud applications.

### Obvious Applications

As should be clear, durable computing makes it easy to create long-running workflows that are executed reliably. Durable computing also makes it possible to coordinate a lot of microservice calls, without any possibility that some of them happen, but others do not (the so-called microservice / API orchestration problem). Durable computing also makes it trivial to automate long-running processes, particularly those involving AI or human interaction.

These are fairly obvious applications of durable computing. However, many of the most fascinating and non-obvious applications of durable computing involve *durable state*.

### Durable State

In any sufficiently powerful durable computing platform (not just Golem Cloud), you can rely on in-memory data structures surviving indefinitely. Well, maybe not indefinitely, but for as long as the worker is running and not killed (which could indeed be forever).

Let's say you have an e-commerce application that keeps track of all activities of each user. Ordinarily, you would store activities of users in a NoSQL database, such as Cassandra or DynamoDB. But in durable computing, you have a new option: store the activities of users in memory (!).

To implement this, you just create one worker for each user (more precisely, configure the API gateway to create one worker for each user based on the */user/{user-id}* route). Then you export some function like the following one to add an activity to a user's history:

def addActivity(activity: Activity) = userActivities.add(activity)

You export other functions to retrieve activities, or perhaps you also have some pattern matching that looks for patterns in user activities and performs some business logic in various cases. The main point is that the user activities are stored not as blobs inside Cassandra or DynamoDB, but as fully typed data structures inside ordinary lists.

You don't have to perform any serialization or deserialization. You don't need a database or an index or a cache. All you need is memory!

Now, this example is a bit naive: if we were storing user activities using one worker per user, then we would at least want wholesale export/import into some semi-structured form, so if we decide to shut down a worker, we can obtain access to the raw data.

But the fact that we can create a massively scalable cloud application for storing user activity data in a couple lines of straightforward code is *stunning*, and one of the most surprising benefits of durable computing.

### Summary

Mainstream durable computing platforms like Golem Cloud are very new, which leaves a lot of developers scratching their heads, wondering if "invincible workers" are even possible.

They are indeed possible, for a precise definition of *invincible*, and they give us a new, incredibly powerful tool for solving some of the hardest problems in the world with code that is **impossibly** simple.

Personally, I'm looking forward to a world in which a single developer could build the next Uber (or other modern tech app) in an afternoon.

We may not be there yet, but durable computing will be indispensable to easily and robustly designing and deploying bulletproof, distributed, stateful cloud apps like Uber and beyond.

To infinity and beyond!
