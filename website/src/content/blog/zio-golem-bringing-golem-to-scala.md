---
title: "ZIO Golem: Bringing Golem to Scala"
date: "2025-12-24"
author: "John A. De Goes"
tags: ["Announcements"]
slug: "zio-golem-bringing-golem-to-scala"
originalUrl: "https://golem.cloud/post/zio-golem-bringing-golem-to-scala"
---

Scala has long been a strong platform for building concurrent and distributed systems.

Frameworks such as Akka established a high bar for actor-based programming on the JVM, offering mature abstractions for concurrency, clustering, and fault tolerance. For many teams, Scala actors have been the foundation for building large-scale, production-grade systems.

At the same time, the JVM itself imposes hard limits on what actor frameworks can achieve.

## JVM limits

JVM actors provide logical isolation, but not physical isolation. All actors ultimately share a process, heap, and runtime. This makes certain capabilities difficult or impossible: lightweight per-actor sandboxing, capability-based security, and running vast numbers of independently isolated actors cheaply.

These are not flaws in actor frameworks. They are consequences of the JVM execution model.

## Golem

Golem is an open-source runtime built on _WebAssembly (WASM)_ that takes a fundamentally different approach to distributed execution.

In Golem, computation happens inside _agents_: lightweight, isolated execution units sandboxed by the WASM runtime. Agents are identified by their constructor parameters, start quickly, and are isolated from one another by construction.

The key architectural difference is that durability is a property of the runtime, not an SDK feature or a programming pattern. You do not implement event sourcing, persistence APIs, or snapshot logic. You write normal stateful code. The runtime automatically handles state persistence, recovery, and exactly-once execution semantics.

This makes Golem fundamentally different from both JVM actor frameworks and traditional serverless platforms.

## The missing piece for Scala

Until now, Golem's SDKs have been available only for Rust and TypeScript. Scala developers could not directly target Golem without leaving their ecosystem.

That changes today.

## ZIO Golem

We are announcing [ZIO Golem](https://github.com/zio/zio-blocks), a new project bringing the Golem runtime to Scala 2.13 and Scala 3.5+.

ZIO Golem is developed by [Ziverge](https://ziverge.com) in collaboration with Matt Hicks, creator of [Typelevel Fabric](https://github.com/typelevel/fabric), and is launching under the ZIO Blocks family of libraries.

Despite the name, ZIO Golem has no dependency on ZIO or any other effect system. Like all ZIO Blocks libraries (including ZIO Schema 2), ZIO Golem works equally well with ZIO, Typelevel, Kyo, or plain Scala using the standard library.

## Code-first agents with macros

ZIO Golem uses Scala macros to deliver a code-first developer experience comparable to Golem's Rust and TypeScript SDKs.

There are no IDLs, no schema files, and no runtime reflection. Agents are defined directly in Scala code, with compile-time validation and code generation.

```scala
@agentDefinition()
trait CounterAgent extends BaseAgent {
  final type AgentInput = String

  @prompt("Increase the count by one")
  @description("Increases the count by one and returns the new value")
  def increment(): Future[Int]
}

object CounterAgent extends AgentCompanion[CounterAgent]
```

The AgentInput type specifies how agents are identified in a cluster and constructed. In this case, each counter agent is identified by a String name.

The macros validate the definition at compile time and generate the necessary code for WASM interop and client access.

## Implementing agents

Agent implementations are defined separately:

```scala
@agentImplementation()
final class CounterAgentImpl(private val name: String) extends CounterAgent {
  private var count: Int = 0

  override def increment(): Future[Int] =
    Future.successful {
      count += 1
      count
    }
}
```

This separation is deliberate. Agent definitions represent stable APIs. Implementations can change independently, without affecting consumers that depend only on the definition.

The count field is automatically durable. If the agent crashes or migrates to another node, it resumes with the same state. No serialization code is required. No database needs to be configured. Durability is provided by the runtime.

## Type-safe distributed calls

Once defined, agents expose a type-safe distributed API:

```scala
val counter = CounterAgent.get("my-counter-123")
val result: Future[Int] = counter.increment()
```

From the Scala developer's perspective, this looks like an ordinary method call returning a Future. Under the hood, Golem provides durable execution, automatic recovery, and exactly-once semantics across failures.

The .get() method has get-or-create semantics. If an agent with that identity already exists, you get a reference to it. If not, it is created.

## Multi-agent systems

Real applications involve multiple agent types collaborating.

```scala
@agentDefinition()
trait OrderAgent extends BaseAgent {
  final type AgentInput = String
  def process(): Future[Receipt]
}

@agentDefinition()
trait PaymentAgent extends BaseAgent {
  final type AgentInput = Unit // Cluster-wide singleton agent
  def charge(amount: BigDecimal, orderId: String): Future[TransactionId]
}

@agentImplementation()
final class OrderAgentImpl(private val orderId: String) extends OrderAgent {
  private var state: OrderState = OrderState.Pending

  override def process(): Future[Receipt] = {
    val payment = PaymentAgent.get(())

    for {
      txId <- payment.charge(calculateTotal(), orderId)
      _    = state = OrderState.Completed
    } yield Receipt(orderId, txId)
  }
}
```

The call to payment.charge() is executed exactly once. If a node crashes during execution, the call is not re-executed on recovery. The result is replayed by the runtime.

## What you get

ZIO Golem inherits the full Golem runtime feature set: durable stateful agents, exactly-once execution semantics, horizontal scalability, WASM-level sandboxing, Kubernetes-native deployment, atomic and immutable deployments, and non-disruptive updates.

These capabilities are not reimplemented in Scala. They are provided by the runtime.

## Why this matters

ZIO Golem combines Scala's expressive type system and macro capabilities with WASM-level isolation and runtime-provided durability.

The result is a new design point for distributed systems: large numbers of isolated, stateful, type-safe agents without JVM overhead or manual persistence logic.

For Scala developers building distributed systems, agent-based architectures, or long-running stateful services, this opens up possibilities that were previously difficult or expensive on the JVM.

## Polyglot systems

Golem already supports Rust and TypeScript. Scala agents join an existing multi-language agent runtime and can communicate with agents written in other languages through the same type-safe RPC mechanism.

## Getting involved

ZIO Golem is under active development.

To follow progress or prepare for early previews, watch the ZIO Blocks repository:

[https://github.com/zio/zio-blocks](https://github.com/zio/zio-blocks)

We're excited to bring Golem to Scala and to see what the Scala community builds with it!
