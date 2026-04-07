# WIT-Friendly Result Helpers

When defining agent methods that surface `result<ok, err>` types, ZIO-Golem provides `golem.runtime.wit.WitResult`.

## Table of Contents

- [Creating Results](#creating-results)
- [Checking Status](#checking-status)
- [Transforming Results](#transforming-results)
- [Extracting Values](#extracting-values)
- [Bridging to WIT](#bridging-to-wit)
- [Interoperability](#interoperability)
- [API Reference](#api-reference)

---

## Creating Results

### Success Values

```scala
import golem.runtime.wit.WitResult

val success: WitResult[Int, Nothing] = WitResult.ok(42)
```

### Error Values

```scala
val failure: WitResult[Nothing, String] = WitResult.err("Something went wrong")
```

### From Existing Types

```scala
// From Either
val fromEither: WitResult[Int, String] = WitResult.fromEither(Right(42))
val fromLeft: WitResult[Int, String] = WitResult.fromEither(Left("error"))

// From Option
val fromSome: WitResult[Int, String] = WitResult.fromOption(Some(42), "was empty")
val fromNone: WitResult[Int, String] = WitResult.fromOption(None, "was empty")
```

---

## Checking Status

```scala
val result: WitResult[Int, String] = WitResult.ok(42)

result.isOk // true
result.isErr // false
```

---

## Transforming Results

### Map Success Values

```scala
val doubled: WitResult[Int, String] =
  WitResult.ok(21).map(_ * 2) // Ok(42)
```

### Map Error Values

```scala
val withPrefix: WitResult[Int, String] =
  WitResult.err("oops").mapError(e => s"Error: $e") // Err("Error: oops")
```

### Chain Operations

```scala
def divide(a: Int, b: Int): WitResult[Int, String] =
  if (b == 0) WitResult.err("division by zero")
  else WitResult.ok(a / b)

val result: WitResult[Int, String] =
  WitResult.ok(100)
    .flatMap(x => divide(x, 2)) // Ok(50)
    .flatMap(x => divide(x, 0)) // Err("division by zero")
```

### Inspect Without Changing

```scala
WitResult.ok(42).tap(value => println(s"Got: $value"))
// Prints: Got: 42
// Returns: Ok(42)
```

---

## Extracting Values

### Pattern Matching

```scala
val result: WitResult[Int, String] = WitResult.ok(42)

val message = result.fold(
  err = e => s"Failed with: $e",
  ok = v => s"Succeeded with: $v"
)
```

### Unwrap (Throws on Mismatch)

```scala
// Get success value (throws on error)
val value: Int = WitResult.ok(42).unwrap()

// Get error value (throws on success)
val error: String = WitResult.err[String]("oops").unwrapErr()
```

---

## Bridging to WIT

### Returning to Host

Use `unwrapForWit` when returning a result through the WIT boundary. If the result is `Err`, the payload is thrown (
mirroring rejected promise behavior):

```scala
import scala.concurrent.Future

def runTask(): Future[Int] = Future.successful {
  val result: WitResult[Int, String] = compute()
  result.unwrapForWit() // Returns Int or throws
}
```

### Error Handling Behavior

| Result Type      | `unwrapForWit()` Behavior       |
|------------------|---------------------------------|
| `Ok(value)`      | Returns `value`                 |
| `Err(throwable)` | Throws the `Throwable` directly |
| `Err(other)`     | Throws `UnwrapError(other)`     |

---

## Interoperability

### Convert to Either

```scala
val either: Either[String, Int] = WitResult.ok(42).toEither
// Right(42)
```

### Convert from Either

```scala
val result: WitResult[Int, String] = WitResult.fromEither(Right(42))
// Ok(42)
```

---

## API Reference

### WitResult[+Ok, +Err]

A sealed trait representing either success (`Ok`) or failure (`Err`).

#### Creation Methods

| Method       | Signature                                                           | Description             |
|--------------|---------------------------------------------------------------------|-------------------------|
| `ok`         | `[Ok](value: Ok): WitResult[Ok, Nothing]`                           | Create a success result |
| `err`        | `[Err](value: Err): WitResult[Nothing, Err]`                        | Create an error result  |
| `fromEither` | `[Err, Ok](either: Either[Err, Ok]): WitResult[Ok, Err]`            | Convert from Either     |
| `fromOption` | `[Ok](value: Option[Ok], orElse: => String): WitResult[Ok, String]` | Convert from Option     |

#### Instance Methods

| Method         | Signature                                                           | Description                       |
|----------------|---------------------------------------------------------------------|-----------------------------------|
| `isOk`         | `Boolean`                                                           | Check if this is a success        |
| `isErr`        | `Boolean`                                                           | Check if this is an error         |
| `map`          | `[B](f: Ok => B): WitResult[B, Err]`                                | Transform success value           |
| `mapError`     | `[F](f: Err => F): WitResult[Ok, F]`                                | Transform error value             |
| `flatMap`      | `[B, Err2 >: Err](f: Ok => WitResult[B, Err2]): WitResult[B, Err2]` | Chain results                     |
| `tap`          | `(f: Ok => Unit): WitResult[Ok, Err]`                               | Inspect success value             |
| `fold`         | `[B](err: Err => B, ok: Ok => B): B`                                | Extract value with handlers       |
| `unwrap`       | `Ok`                                                                | Extract success (throws on error) |
| `unwrapErr`    | `Err`                                                               | Extract error (throws on success) |
| `unwrapForWit` | `Ok`                                                                | Extract for WIT boundary          |
| `toEither`     | `Either[Err, Ok]`                                                   | Convert to Either                 |

---

## Complete Example

```scala
import golem.runtime.wit.WitResult
import scala.concurrent.Future

// Define operations that return WitResult
def parseNumber(s: String): WitResult[Int, String] =
  try WitResult.ok(s.toInt)
  catch {
    case _: NumberFormatException => WitResult.err(s"Invalid number: $s")
  }

def divide(a: Int, b: Int): WitResult[Int, String] =
  if (b == 0) WitResult.err("Division by zero")
  else WitResult.ok(a / b)

// Compose operations
def compute(input: String): WitResult[Int, String] =
  for {
    num <- parseNumber(input)
    result <- divide(100, num)
  } yield result

// Use in an agent method
def processInput(input: String): Future[Int] = Future.successful {
  compute(input).unwrapForWit()
}
```
