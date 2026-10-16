# Transaction Helpers

ZIO-Golem provides transaction helpers under `golem.Transactions`.

## Table of Contents

- [Core Concepts](#core-concepts)
- [Infallible Transactions](#infallible-transactions)
- [Fallible Transactions](#fallible-transactions)
- [Defining Operations](#defining-operations)
- [Error Handling](#error-handling)
- [Best Practices](#best-practices)

---

## Core Concepts

### What Are Transactions?

Transactions group a sequence of operations where:

1. Each operation has an **execute** step and a **compensate** step
2. When an operation returns a domain error, previous compensations run in reverse order
3. An infallible transaction starts a fresh attempt; a fallible transaction returns the error with rollback status

### Transaction Types

| Type           | Behavior on Failure               | Return Type                                        |
|----------------|-----------------------------------|----------------------------------------------------|
| **Infallible** | Auto-retry until success          | `Future[A]`                                        |
| **Fallible**   | Return error with rollback status | `Future[Either[TransactionFailure[Err], A]]`       |

---

## Infallible Transactions

Use `infallibleTransaction` when operations must eventually succeed. When an operation returns `Left`, compensations
execute and the entire transaction retries automatically.

These helpers implement sagas, not atomic regions. Attempts and compensations append to the oplog.
They do not rewind execution or undo concurrent work. An unexpected failed `Future` traps without
running compensations and fails the invocation.
The guest-code loop has no built-in attempt bound or backoff guarantee and does not use the host
retry policy. A permanent domain failure can loop forever; eventual success is not guaranteed.

```scala
import golem.Transactions
import scala.concurrent.Future

val result: Future[Int] = Transactions.infallibleTransaction { tx =>
  // Define an operation with its compensation
  val createResource = Transactions.operation[Unit, Int, String](
    // Execute: create the resource, return its ID
    _ => Future.successful(Right(42))
  )(
    // Compensate: delete the resource on rollback
    (_, resourceId) => {
      deleteResource(resourceId)
      Future.successful(Right(()))
    }
  )

  // Execute the operation
  for {
    resourceId <- tx.execute(createResource, ())
  } yield resourceId
}
```

### How Infallible Transactions Work

1. A fresh transaction context is created
2. Each `tx.execute` runs the operation
3. On success, compensation is registered
4. On failure:
    - All compensations run in reverse order
    - Transaction retries from the beginning with a fresh context
    - Prior attempts and compensations remain in the oplog

---

## Fallible Transactions

Use `fallibleTransaction` when you want to handle failures explicitly without automatic retry.

```scala
import golem.Transactions
import golem.Transactions.TransactionFailure
import scala.concurrent.Future

val result: Future[Either[TransactionFailure[String], Int]] =
  Transactions.fallibleTransaction[Int, String] { tx =>
    val increment = Transactions.operation[Int, Int, String](
      in => Future.successful(Right(in + 1))
    )(
      (_, _) => Future.successful(Right(()))
    )

    for {
      a <- tx.execute(increment, 1)
      b <- a match {
        case Right(v) => tx.execute(increment, v)
        case left     => Future.successful(left)
      }
      c <- b match {
        case Right(v) => tx.execute(increment, v)
        case left     => Future.successful(left)
      }
    } yield c.flatMap(v => Right(v))
  }

// Handle the result
result.foreach {
  case Right(value) =>
    println(s"Success: $value")

  case Left(TransactionFailure.FailedAndRolledBackCompletely(err)) =>
    println(s"Failed but rolled back cleanly: $err")

  case Left(TransactionFailure.FailedAndRolledBackPartially(err, compErr)) =>
    println(s"Failed with partial rollback: $err, compensation error: $compErr")
}
```

### TransactionFailure Variants

| Variant                                                  | Meaning                        |
|----------------------------------------------------------|--------------------------------|
| `FailedAndRolledBackCompletely(error)`                   | All compensations succeeded    |
| `FailedAndRolledBackPartially(error, compensationError)` | Some compensations also failed |

---

## Defining Operations

Operations bundle an execute function with its compensation:

```scala
val operation = Transactions.operation[In, Out, Err](
  // Execute: In => Future[Either[Err, Out]]
  run = (input: In) => {
    // Perform the operation
    // Return Future(Right(result)) on success
    // Return Future(Left(error)) on failure
    Future.successful(Right(result))
  }
)(
  // Compensate: (In, Out) => Future[Either[Err, Unit]]
  compensate = (input: In, output: Out) => {
    // Undo the operation
    // Has access to both input and successful output
    Future.successful(Right(()))
  }
)
```

### Operation Type Signature

```scala
trait Operation[-In, Out, Err] {
  def execute(input: In): Future[Either[Err, Out]]

  def compensate(input: In, output: Out): Future[Either[Err, Unit]]
}
```

---

## Error Handling

### In Infallible Transactions

Compensation failures throw `IllegalStateException`:

```scala
Transactions.infallibleTransaction { tx =>
  val op = Transactions.operation[Unit, Int, String](
    _ => Future.successful(Right(42))
  )(
    (_, _) => Future.successful(Left("Compensation failed!")) // Throws on failure
  )

  tx.execute(op, ())
}
```

### In Fallible Transactions

Compensation failures are captured in the return type:

```scala
Transactions.fallibleTransaction[Int, String] { tx =>
  // If compensation fails, result is FailedAndRolledBackPartially
  ???
}
```

---

## Best Practices

### 1. Keep Compensations Simple

Compensations should be idempotent and unlikely to fail:

```scala
// Good: Simple delete that tolerates non-existence
val deleteOp = Transactions.operation[Unit, String, String](
  _ => createFile()
)(
  (_, path) => {
    if (fileExists(path)) deleteFile(path)
    Future.successful(Right(()))
  }
)

// Bad: Complex logic in compensation
val badOp = Transactions.operation[Unit, String, String](
  _ => createFile()
)(
  (_, path) => {
    // Don't do complex operations here
    archiveFile(path)
    notifySystem()
    updateDatabase()
    Future.successful(Right(()))
  }
)
```

### 2. Order Operations by Reversibility

Put easily-reversible operations first:

```scala
Transactions.fallibleTransaction[Unit, String] { tx =>
  for {
    // Easy to reverse (just delete)
    _ <- tx.execute(createTempFile, ())
    // Harder to reverse (external API)
    _ <- tx.execute(sendNotification, ())
    // Hardest to reverse (payment)
    r <- tx.execute(processPayment, ())
  } yield r
}
```

### 3. Use Infallible for Critical Paths

When an operation *must* succeed (like cleanup), use infallible:

```scala
Transactions.infallibleTransaction { tx =>
  // Will retry until cleanup completes
  tx.execute(cleanupResources, ())
}
```

### 4. Log Transaction Boundaries

For debugging, log when transactions start and complete:

```scala
Transactions.fallibleTransaction[Int, String] { tx =>
  console.log("Transaction started")
  tx.execute(operation, input).map { result =>
    console.log(s"Transaction completed: $result")
    result
  }
}
```

---

## Integration with Host

Both helpers use ordinary durable host calls. Each retry appends fresh operations after the
compensations of the previous attempt. Operations must provide their own compensation semantics;
the helpers do not provide isolation from concurrent invocations.
