/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime.wit

/**
 * A WIT-friendly result type that mirrors the ergonomics of the JS SDK. SDK's
 * `Result`.
 *
 * Use this when defining agent methods that surface `result<ok, err>` values to
 * the host. It operates like Scala's `Either`, but includes helpers
 * (`unwrapForWit`, `mapError`, etc.) tailored for the Scala.js ↔ WIT bridge.
 *
 * ==Creating Results==
 * {{{
 * val success: WitResult[Int, Nothing] = WitResult.ok(42)
 * val failure: WitResult[Nothing, String] = WitResult.err("boom")
 * val fromEither: WitResult[Int, String] = WitResult.fromEither(Right(1))
 * }}}
 *
 * ==Bridging to WIT==
 * Use `unwrapForWit` when returning to the host:
 * {{{
 * def runTask(): Future[Int] = Future.successful(compute().unwrapForWit())
 * }}}
 *
 * @tparam Ok
 *   The success type
 * @tparam Err
 *   The error type
 */
sealed trait WitResult[+Ok, +Err] { self =>

  /** Returns `true` if this is an error result. */
  def isErr: Boolean = !isOk

  /** Returns `true` if this is a success result. */
  def isOk: Boolean =
    fold(_ => false, _ => true)

  /** Converts this result to a Scala `Either`. */
  def toEither: Either[Err, Ok] =
    fold(err => Left(err), value => Right(value))

  /**
   * Transforms the success value using the given function.
   *
   * @param f
   *   Function to apply to the success value
   * @return
   *   A new result with the transformed success value
   */
  def map[B](f: Ok => B): WitResult[B, Err] =
    fold(err => WitResult.Err(err), ok => WitResult.Ok(f(ok)))

  /**
   * Transforms the error value using the given function.
   *
   * @param f
   *   Function to apply to the error value
   * @return
   *   A new result with the transformed error value
   */
  def mapError[F](f: Err => F): WitResult[Ok, F] =
    fold(err => WitResult.Err(f(err)), ok => WitResult.Ok(ok))

  /**
   * Chains another result-producing operation.
   *
   * @param f
   *   Function that produces another WitResult
   * @return
   *   The result of the chained operation, or the original error
   */
  def flatMap[B, Err2 >: Err](f: Ok => WitResult[B, Err2]): WitResult[B, Err2] =
    fold(err => WitResult.Err(err), ok => f(ok))

  /**
   * Inspects the success value without altering the result.
   *
   * @param f
   *   Side-effecting function called with the success value
   * @return
   *   This result unchanged
   */
  def tap(f: Ok => Unit): WitResult[Ok, Err] = {
    fold(_ => (), ok => f(ok))
    self
  }

  /**
   * Extracts the value using handlers for both cases.
   *
   * @param err
   *   Handler for error case
   * @param ok
   *   Handler for success case
   * @return
   *   The result of the appropriate handler
   */
  def fold[B](err: Err => B, ok: Ok => B): B

  /**
   * Extracts the success value, throwing on error.
   *
   * @return
   *   The success value
   * @throws UnwrapError
   *   if this is an error result
   */
  def unwrap(): Ok =
    fold(err => throw UnwrapError(err), identity)

  /**
   * Extracts the error value, throwing on success.
   *
   * @return
   *   The error value
   * @throws java.lang.IllegalStateException
   *   if this is a success result
   */
  def unwrapErr(): Err =
    fold(identity, ok => throw new IllegalStateException(s"unwrapErr called on Ok($ok)"))

  /**
   * Extracts the success value for WIT boundary crossing.
   *
   * When exporting a `result` back through the WIT boundary (e.g., returning to
   * host JS), call this to either return the success value or throw the error
   * payload. This mirrors the JS SDK behavior where `Result.err` triggers a
   * rejected promise.
   *
   * @return
   *   The success value
   * @throws java.lang.Throwable
   *   error payload (as-is if Throwable, wrapped otherwise)
   */
  def unwrapForWit(): Ok =
    fold(err => throw WitResult.unwrapPayload(err), identity)
}

/**
 * Companion object providing factory methods and case classes for
 * [[WitResult]].
 */
object WitResult {

  /**
   * Creates a WitResult from a Scala Either.
   *
   * @param either
   *   The Either to convert
   * @return
   *   A WitResult with the same semantics
   */
  def fromEither[Err, Ok](either: Either[Err, Ok]): WitResult[Ok, Err] =
    either.fold(err, ok)

  /**
   * Creates a WitResult from an Option, using a default error message.
   *
   * @param value
   *   The Option to convert
   * @param orElse
   *   Error message if the Option is None
   * @return
   *   A success result for Some, error result for None
   */
  def fromOption[Ok](value: Option[Ok], orElse: => String): WitResult[Ok, String] =
    value match {
      case Some(result) => ok(result)
      case None         => err(orElse)
    }

  /**
   * Creates a successful result.
   *
   * @param value
   *   The success value
   * @return
   *   A WitResult containing the success value
   */
  def ok[Ok](value: Ok): WitResult[Ok, Nothing] = Ok(value)

  /**
   * Creates an error result.
   *
   * @param value
   *   The error value
   * @return
   *   A WitResult containing the error value
   */
  def err[Err](value: Err): WitResult[Nothing, Err] = Err(value)

  private[wit] def unwrapPayload(payload: Any): Throwable =
    payload match {
      case throwable: Throwable => throwable
      case other                => UnwrapError(other)
    }

  /**
   * Represents a successful result.
   *
   * @param value
   *   The success value
   */
  final case class Ok[+Ok](value: Ok) extends WitResult[Ok, Nothing] {
    override def fold[B](err: Nothing => B, ok: Ok => B): B = ok(value)

    override def toString: String = s"Ok($value)"
  }

  /**
   * Represents an error result.
   *
   * @param value
   *   The error value
   */
  final case class Err[+Err](value: Err) extends WitResult[Nothing, Err] {
    override def fold[B](err: Err => B, ok: Nothing => B): B = err(value)

    override def toString: String = s"Err($value)"
  }
}

final case class UnwrapError(payload: Any) extends RuntimeException(s"WitResult.unwrap called on Err($payload)")
