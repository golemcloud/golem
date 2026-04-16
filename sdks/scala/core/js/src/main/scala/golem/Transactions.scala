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

package golem

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

/**
 * Transaction helpers for managing atomic operations with automatic rollback.
 *
 * This mirrors the JS SDK's `infallibleTransaction` and `fallibleTransaction`
 * helpers, providing the same ergonomics for Scala.js agents.
 *
 * ==Infallible Transactions==
 *
 * Use [[infallibleTransaction]] when operations must eventually succeed. On
 * failure, compensations run and the transaction retries automatically:
 *
 * {{{
 * val result = Transactions.infallibleTransaction { tx =>
 *   val op = Transactions.operation[Unit, Int, String](
 *     _ => Future.successful(Right(42))
 *   )((_, _) => Future.successful(Right(())))
 *   tx.execute(op, ())
 * }
 * }}}
 *
 * ==Fallible Transactions==
 *
 * Use [[fallibleTransaction]] when you want explicit error handling:
 *
 * {{{
 * val result = Transactions.fallibleTransaction[String, Int] { tx =>
 *   val op = Transactions.operation[Int, Int, String](
 *     in => Future.successful(Right(in + 1))
 *   )((_, _) => Future.successful(Right(())))
 *   tx.execute(op, 1)
 * }
 * }}}
 *
 * @see
 *   [[docs/transactions.md]] for detailed usage patterns
 */
/**
 * Transaction helpers for managing atomic operations with automatic rollback.
 */
object Transactions {

  /**
   * Creates an operation from execute and compensate functions.
   *
   * This is a convenience method equivalent to [[Operation.apply]].
   *
   * @param run
   *   The forward operation
   * @param compensate
   *   The rollback operation
   */
  def operation[In, Out, Err](
    run: In => Future[Either[Err, Out]]
  )(
    compensate: (In, Out) => Future[Either[Err, Unit]]
  ): Operation[In, Out, Err] =
    Operation(run, compensate)

  /**
   * Runs an infallible transaction that retries on failure.
   *
   * When any operation fails:
   *   1. All registered compensations run in reverse order
   *      2. The oplog index resets to the transaction start
   *      3. The entire transaction body re-executes
   *
   * The transaction keeps retrying until all operations succeed.
   *
   * @param body
   *   The transaction body receiving the transaction context
   * @return
   *   A Future containing the result of a successful transaction run
   */
  def infallibleTransaction[A](body: InfallibleTransaction => Future[A]): Future[A] = {
    def loop(): Future[A] = {
      val guard = Guards.markAtomicOperation()
      val begin = HostApi.getOplogIndex()
      val tx    = new InfallibleTransaction
      body(tx).transform(
        result => {
          guard.drop()
          result
        },
        {
          case RetrySignal =>
            HostApi.setOplogIndex(begin)
            guard.drop()
            // Cannot directly recurse here in transform's failure handler,
            // so we throw and catch below
            throw RetrySignal
          case other =>
            // Do NOT drop — leave the atomic region open so the executor
            // sees the trap inside the region and can retry from the begin marker.
            throw other
        }
      ).recoverWith { case RetrySignal =>
        loop()
      }
    }

    loop()
  }

  /**
   * Runs a fallible transaction that returns errors instead of retrying.
   *
   * When an operation fails:
   *   1. Registered compensations run in reverse order (best-effort)
   *      2. The failure is returned with rollback status
   *
   * @param body
   *   The transaction body receiving the transaction context
   * @return
   *   A Future containing either a failure description or the successful result
   */
  def fallibleTransaction[A, Err](
    body: FallibleTransaction[Err] => Future[Either[Err, A]]
  ): Future[Either[TransactionFailure[Err], A]] = {
    val guard = Guards.markAtomicOperation()
    val tx    = new FallibleTransaction[Err]
    body(tx).flatMap {
      case Right(value) =>
        guard.drop()
        Future.successful(Right(value))
      case Left(err) =>
        tx.onFailure(err).map { failure =>
          guard.drop()
          Left(failure)
        }.recover { case e =>
          // Do NOT drop — leave the atomic region open so the executor
          // sees the trap inside the region and can retry from the begin marker.
          throw e
        }
    }.recover { case e =>
      // Do NOT drop — leave the atomic region open for retry.
      throw e
    }
  }

  /**
   * Describes how a fallible transaction failed.
   *
   * @tparam Err
   *   The error type from operations
   */
  sealed trait TransactionFailure[+Err]

  /**
   * Represents an atomic operation with execute and compensate steps.
   *
   * @tparam In
   *   Input type for the operation
   * @tparam Out
   *   Output type on success
   * @tparam Err
   *   Error type on failure
   */
  trait Operation[-In, Out, Err] {

    /** Executes the operation, returning a Future of either an error or the result. */
    def execute(input: In): Future[Either[Err, Out]]

    /** Compensates (undoes) a successful operation during rollback. */
    def compensate(input: In, output: Out): Future[Either[Err, Unit]]
  }

  /**
   * Transaction context for infallible transactions.
   *
   * Operations that fail trigger automatic rollback and retry.
   */
  final class InfallibleTransaction private[golem] () {
    private var compensations: List[() => Future[Unit]] = Nil

    /**
     * Executes an operation within the transaction.
     *
     * On success, registers the compensation for potential rollback. On
     * failure, runs all compensations and signals retry.
     *
     * @param operation
     *   The operation to execute
     * @param input
     *   The input to the operation
     * @return
     *   A Future containing the operation result (signals retry on failure)
     */
    def execute[In, Out, Err](operation: Operation[In, Out, Err], input: In): Future[Out] =
      operation.execute(input).flatMap {
        case Right(value) =>
          compensations ::= (() =>
            operation.compensate(input, value).map {
              case Right(_)     => ()
              case Left(reason) => throw new IllegalStateException(s"Infallible compensation failed: $reason")
            }
          )
          Future.successful(value)
        case Left(_) =>
          rollback()
      }

    private def rollback(): Future[Nothing] = {
      def runCompensations(remaining: List[() => Future[Unit]]): Future[Unit] =
        remaining match {
          case Nil          => Future.successful(())
          case head :: tail => head().flatMap(_ => runCompensations(tail))
        }

      runCompensations(compensations).flatMap(_ => Future.failed(RetrySignal))
    }
  }

  /**
   * Transaction context for fallible transactions.
   *
   * Operations that fail trigger best-effort rollback without retry.
   */
  final class FallibleTransaction[Err] private[golem] () {
    private var compensations: List[() => Future[Either[Err, Unit]]] = Nil

    /**
     * Executes an operation within the transaction.
     *
     * On success, registers the compensation for potential rollback. On
     * failure, returns the error (call [[onFailure]] to trigger rollback).
     *
     * @param operation
     *   The operation to execute
     * @param input
     *   The input to the operation
     * @return
     *   A Future containing either an error or the operation result
     */
    def execute[In, Out](
      operation: Operation[In, Out, Err],
      input: In
    ): Future[Either[Err, Out]] =
      operation.execute(input).map {
        case Right(output) =>
          compensations ::= (() => operation.compensate(input, output))
          Right(output)
        case left =>
          left
      }

    /**
     * Triggers rollback after a failure.
     *
     * Runs all registered compensations in reverse order.
     *
     * @param error
     *   The error that caused the failure
     * @return
     *   A Future containing a description of how the rollback completed
     */
    def onFailure(error: Err): Future[TransactionFailure[Err]] = {
      def compensateLater(pending: List[() => Future[Either[Err, Unit]]]): Future[Option[Err]] =
        pending match {
          case Nil => Future.successful(None)
          case head :: tail =>
            head().flatMap {
              case Right(_)        => compensateLater(tail)
              case Left(compError) => Future.successful(Some(compError))
            }
        }

      compensateLater(compensations).map {
        case Some(compError) =>
          TransactionFailure.FailedAndRolledBackPartially(error, compError)
        case None =>
          TransactionFailure.FailedAndRolledBackCompletely(error)
      }
    }
  }

  object Operation {

    /**
     * Creates an operation from execute and compensate functions.
     *
     * @param run
     *   The forward operation
     * @param compensateFn
     *   The rollback operation, given original input and output
     */
    def apply[In, Out, Err](
      run: In => Future[Either[Err, Out]],
      compensateFn: (In, Out) => Future[Either[Err, Unit]]
    ): Operation[In, Out, Err] =
      new Operation[In, Out, Err] {
        override def execute(input: In): Future[Either[Err, Out]] = run(input)

        override def compensate(input: In, output: Out): Future[Either[Err, Unit]] = compensateFn(input, output)
      }
  }

  object TransactionFailure {

    /**
     * The transaction failed, but all compensations succeeded.
     *
     * @param error
     *   The error that caused the transaction to fail
     */
    final case class FailedAndRolledBackCompletely[Err](error: Err) extends TransactionFailure[Err]

    /**
     * The transaction failed, and some compensations also failed.
     *
     * @param error
     *   The original error
     * @param compensationFailure
     *   The error from a failed compensation
     */
    final case class FailedAndRolledBackPartially[Err](error: Err, compensationFailure: Err)
        extends TransactionFailure[Err]
  }

  private case object RetrySignal extends Throwable with scala.util.control.NoStackTrace
}
