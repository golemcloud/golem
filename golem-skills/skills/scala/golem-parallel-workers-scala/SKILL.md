---
name: golem-parallel-workers-scala
description: "Fan out work to multiple parallel agents and collect results in a Scala Golem project. Use when the user asks about parallel execution, fan-out/fan-in, spawning child agents for parallel work, forking, or aggregating results from multiple agents."
---

# Parallel Workers — Fan-Out / Fan-In (Scala)

## Overview

Golem agents process invocations **sequentially** — a single agent cannot run work in parallel. To execute work concurrently, distribute it across **multiple agent instances**. This skill covers two approaches:

1. **Child agents via codegen-generated `XClient.get(id)`** — spawn separate agent instances, dispatch work, and collect results
2. **`HostApi.fork()`** — clone the current agent at the current execution point for lightweight parallel execution

## Approach 1: Child Agent Fan-Out

Spawn child agents, dispatch work, and collect results using `Future.sequence` or Golem promises.

### Basic Pattern with Future.sequence

```scala
import golem.*
import golem.runtime.annotations.{agentDefinition, agentImplementation}

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

@agentDefinition()
trait Coordinator extends BaseAgent {
  class Id()
  def fanOut(items: List[String]): Future[List[String]]
}

@agentImplementation()
class CoordinatorImpl() extends Coordinator {
  override def fanOut(items: List[String]): Future[List[String]] = {
    // Spawn one child per item and call concurrently
    val futures = items.zipWithIndex.map { case (item, i) =>
      val child = WorkerClient.get(i)
      child.process(item)
    }

    // Wait for all children to finish
    Future.sequence(futures)
  }
}

@agentDefinition()
trait Worker extends BaseAgent {
  class Id(val id: Int)
  def process(data: String): Future[String]
}

@agentImplementation()
class WorkerImpl(private val id: Int) extends Worker {
  override def process(data: String): Future[String] = {
    Future.successful(s"processed-$data")
  }
}
```

### Fire-and-Forget with Promise Collection

For long-running work, trigger children with fire-and-forget and collect results via Golem promises:

```scala
import golem.HostApi
import golem.HostApi.PromiseId
import zio.blocks.schema.Schema

case class WorkResult(value: String) derives Schema

@agentImplementation()
class CoordinatorImpl() extends Coordinator {
  override def dispatchAndCollect(regions: List[String]): Future[List[String]] = {
    // Create one promise per child
    val promiseIds = regions.map(_ => HostApi.createPromise())

    // Fire-and-forget: trigger each child with its promise ID
    regions.zip(promiseIds).foreach { case (region, pid) =>
      val child = RegionWorkerClient.get(region)
      child.runReport.trigger(pid)
    }

    // Collect all results (agent suspends on each until completed)
    val futures = promiseIds.map { pid =>
      HostApi.awaitPromiseJson[WorkResult](pid).map(_.value)
    }
    Future.sequence(futures)
  }
}

@agentImplementation()
class RegionWorkerImpl(private val region: String) extends RegionWorker {
  override def runReport(promiseId: PromiseId): Future[Unit] = {
    val result = WorkResult(s"Report for $region: OK")
    HostApi.completePromiseJson(promiseId, result)
    Future.successful(())
  }
}
```

### Chunked Fan-Out

Batch children to limit concurrency:

```scala
override def fanOutChunked(items: List[String]): Future[List[String]] = {
  val chunks = items.grouped(5).toList

  chunks.foldLeft(Future.successful(List.empty[String])) { (accFut, chunk) =>
    accFut.flatMap { acc =>
      val futures = chunk.zipWithIndex.map { case (item, i) =>
        WorkerClient.get(i).process(item)
      }
      Future.sequence(futures).map(acc ++ _)
    }
  }
}
```

### Error Handling

Use `Future.traverse` with `recover` for partial failure handling:

```scala
override def fanOutWithErrors(items: List[String]): Future[(List[String], List[String])] = {
  val futures = items.zipWithIndex.map { case (item, i) =>
    WorkerClient.get(i).process(item)
      .map(Right(_))
      .recover { case e: Throwable => Left(s"Item $item failed: ${e.getMessage}") }
  }

  Future.sequence(futures).map { results =>
    val successes = results.collect { case Right(v) => v }
    val failures = results.collect { case Left(e) => e }
    (successes, failures)
  }
}
```

## Approach 2: `HostApi.fork()`

`HostApi.fork()` clones the current agent at the current execution point, creating a new agent instance with the same state but a unique phantom ID. Use Golem promises to synchronize between the original and forked agents.

### Basic Fork Pattern

```scala
import golem.HostApi
import golem.HostApi.{ForkResult, PromiseId}

override def parallelCompute(): Future[String] = {
  val promiseId = HostApi.createPromise()

  HostApi.fork() match {
    case ForkResult.Original(_) =>
      // Wait for the forked agent to complete the promise
      HostApi.awaitPromise(promiseId).map { bytes =>
        val forkedResult = new String(bytes, "UTF-8")
        s"Combined: original + $forkedResult"
      }

    case ForkResult.Forked(_) =>
      // Do work in the forked copy
      val result = "forked-result"
      HostApi.completePromise(promiseId, result.getBytes("UTF-8"))
      Future.successful("forked done") // Only seen by the forked agent
  }
}
```

### Multi-Fork Fan-Out

Fork multiple times for N-way parallelism:

```scala
override def multiFork(n: Int): Future[List[String]] = {
  val promiseIds = (0 until n).map(_ => HostApi.createPromise()).toList

  for (i <- 0 until n) {
    HostApi.fork() match {
      case ForkResult.Original(_) =>
        // Continue to next fork

      case ForkResult.Forked(_) =>
        // Each forked agent does its slice of work
        val output = s"result-from-fork-$i"
        HostApi.completePromise(promiseIds(i), output.getBytes("UTF-8"))
        return Future.successful(Nil) // Forked agent exits here
    }
  }

  // Original agent collects all results
  val futures = promiseIds.map { pid =>
    HostApi.awaitPromise(pid).map(bytes => new String(bytes, "UTF-8"))
  }
  Future.sequence(futures)
}
```

## When to Use Which Approach

| Criteria | Child Agents | `HostApi.fork()` |
|----------|-------------|------------------|
| Work is **independent** and stateless | ✅ Best fit | Works but overkill |
| Need to **share current state** with workers | ❌ Must pass via args | ✅ Forked copy inherits state |
| Workers need **persistent identity** | ✅ Each has own ID | ❌ Forked agents are ephemeral phantoms |
| Number of parallel tasks is **dynamic** | ✅ Spawn as many as needed | ✅ Fork in a loop |
| Need **simple error isolation** | ✅ Child failure doesn't crash parent | ⚠️ Forked agent shares oplog lineage |

## Key Points

- **No threads**: Golem is single-threaded per agent — parallelism is achieved by distributing across agent instances
- **Durability**: All RPC calls, promises, and fork operations are durably recorded — work survives crashes
- **Deadlock avoidance**: Never have two agents awaiting each other synchronously — use `.trigger` to break cycles
- **Cleanup**: Child agents persist after the coordinator finishes; delete them explicitly if they hold unwanted state
- **Aggregation**: Use `Future.sequence` to collect results from multiple `Future`s, or iterate over promise IDs with `HostApi.awaitPromise`/`awaitPromiseJson`
