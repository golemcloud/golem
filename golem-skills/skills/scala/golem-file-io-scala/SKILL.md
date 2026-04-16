---
name: golem-file-io-scala
description: "Reading and writing files from a Scala Golem agent. Use when the user asks to read files, write files, or do filesystem operations from agent code in Scala."
---

# File I/O in Scala Golem Agents

## Overview

Golem Scala agents are compiled to JavaScript via Scala.js and run in a QuickJS-based WASM runtime. The runtime provides `node:fs` for filesystem operations, accessible via Scala.js JavaScript interop. Standard JVM file I/O (`java.io.File`, `java.nio.file.*`) is **not available**.

To provision files into an agent's filesystem, load the `golem-add-initial-files` skill. To understand the full runtime environment, load the `golem-js-runtime` skill.

## Setting Up the `node:fs` Facade

Define a Scala.js facade object for the `node:fs` module:

```scala
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

@js.native
@JSImport("node:fs", JSImport.Namespace)
private object Fs extends js.Object {
  def readFileSync(path: String, encoding: String): String = js.native
  def readFileSync(path: String): js.typedarray.Uint8Array = js.native
  def writeFileSync(path: String, data: String): Unit = js.native
  def existsSync(path: String): Boolean = js.native
  def readdirSync(path: String): js.Array[String] = js.native
  def appendFileSync(path: String, data: String): Unit = js.native
  def mkdirSync(path: String, options: js.Object): Unit = js.native
}
```

> **Important:** WASI modules like `node:fs` are **not** available during the build-time pre-initialization (wizer) phase — they are only available at runtime. Use **lazy val** to defer initialization:
>
> ```scala
> // ✅ CORRECT — lazy val defers import to first runtime use
> private lazy val fs: Fs.type = Fs
>
> // ❌ WRONG — top-level val triggers import during pre-initialization and fails
> private val fs: Fs.type = Fs
> ```

## Reading Files

### Text Files

```scala
val content: String = Fs.readFileSync("/data/config.json", "utf-8")
```

### Binary Files

```scala
val bytes: js.typedarray.Uint8Array = Fs.readFileSync("/data/image.png")
```

## Writing Files

Only files provisioned with `read-write` permission (or files in non-provisioned paths) can be written to.

```scala
Fs.writeFileSync("/tmp/output.txt", "Hello, world!")
```

## Checking File Existence

```scala
if (Fs.existsSync("/data/config.json")) {
  val content = Fs.readFileSync("/data/config.json", "utf-8")
}
```

## Listing Directories

```scala
val files: js.Array[String] = Fs.readdirSync("/data")
files.foreach(println)
```

## Complete Agent Example

```scala
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.BaseAgent
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport
import scala.concurrent.Future

@js.native
@JSImport("node:fs", JSImport.Namespace)
private object Fs extends js.Object {
  def readFileSync(path: String, encoding: String): String = js.native
  def appendFileSync(path: String, data: String): Unit = js.native
}

@agentDefinition()
trait FileReaderAgent extends BaseAgent {
  class Id(val name: String)
  def readGreeting(): Future[String]
  def writeLog(message: String): Future[Unit]
}

@agentImplementation()
final class FileReaderAgentImpl(private val name: String) extends FileReaderAgent {

  override def readGreeting(): Future[String] = Future.successful {
    Fs.readFileSync("/data/greeting.txt", "utf-8").trim
  }

  override def writeLog(message: String): Future[Unit] = Future.successful {
    Fs.appendFileSync("/tmp/agent.log", message + "\n")
  }
}
```

## Key Constraints

- Use `node:fs` via `@JSImport` — standard JVM file I/O (`java.io.File`, `java.nio.file.*`) does **not** work in Scala.js
- Use **lazy val** or defer `node:fs` access to method bodies to avoid pre-initialization failures
- Files provisioned via `golem-add-initial-files` with `read-only` permission cannot be written to
- The filesystem is per-agent-instance — each agent has its own isolated filesystem
- File changes within an agent are persistent across invocations (durable state)
