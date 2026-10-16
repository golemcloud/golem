/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem

import golem.host.RetryApi
import golem.host.js.JsPredicateValue
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport
import scala.scalajs.js.typedarray.Uint8Array
import scala.util.control.NonFatal

/**
 * Sagas append attempts and compensations to the oplog; they never rewind it.
 */
object Transactions {
  trait Operation[-In, Out, Err] {
    def execute(input: In): Future[Either[Err, Out]]
    def compensate(input: In, output: Out): Future[Either[Err, Unit]]
  }

  object Operation {
    def apply[In, Out, Err](
      run: In => Future[Either[Err, Out]],
      compensateFn: (In, Out) => Future[Either[Err, Unit]]
    ): Operation[In, Out, Err] = new Operation[In, Out, Err] {
      def execute(input: In): Future[Either[Err, Out]]                  = run(input)
      def compensate(input: In, output: Out): Future[Either[Err, Unit]] = compensateFn(input, output)
    }
  }

  def operation[In, Out, Err](run: In => Future[Either[Err, Out]])(
    compensate: (In, Out) => Future[Either[Err, Unit]]
  ): Operation[In, Out, Err] = Operation(run, compensate)

  sealed trait TransactionFailure[+Err]
  object TransactionFailure {
    final case class FailedAndRolledBackCompletely[Err](error: Err) extends TransactionFailure[Err]
    final case class FailedAndRolledBackPartially[Err](error: Err, compensationFailure: Any)
        extends TransactionFailure[Err]
    final case class RetryExhausted(error: Any)                        extends TransactionFailure[Nothing]
    final case class RetryEvaluationFailed(error: Any, reason: String) extends TransactionFailure[Nothing]
  }

  /**
   * Keeps the original failure and compensation evidence on a thrown failure.
   */
  final case class PartialRollbackException(originalFailure: Throwable, compensationFailure: Any)
      extends RuntimeException(
        s"Saga compensation failed: $compensationFailure",
        originalFailure
      )

  private final case class OperationFailure(error: Any) extends RuntimeException with scala.util.control.NoStackTrace

  // Future.delegate and Try exclude InterruptedException. Catch it at user boundaries too.
  private def invoke[A](body: => Future[A]): Future[A] =
    try body
    catch {
      case error: InterruptedException => Future.failed(error)
      case NonFatal(error)             => Future.failed(error)
    }

  private def original(error: Throwable): Throwable = error match {
    case wrapped: java.util.concurrent.ExecutionException if wrapped.getCause != null => wrapped.getCause
    case other                                                                        => other
  }

  /**
   * Returns domain errors, but compensates and rethrows catchable thrown
   * failures.
   */
  def fallibleTransaction[A, Err](
    body: FallibleTransaction[Err] => Future[Either[Err, A]]
  ): Future[Either[TransactionFailure[Err], A]] = {
    val tx = new FallibleTransaction[Err]
    invoke(body(tx)).transformWith {
      case scala.util.Success(Right(value)) => Future.successful(Right(value))
      case scala.util.Success(Left(error))  => tx.onFailure(error).map(Left(_))
      case scala.util.Failure(error)        =>
        val cause = original(error)
        tx.rollback().flatMap {
          case None               => Future.failed(cause)
          case Some(compensation) => Future.failed(PartialRollbackException(cause, compensation))
        }
    }
  }

  /**
   * Retries fully compensated failures with the host policy and durable
   * clock/timer. None of the operation errors or caught causes are discarded on
   * exhaustion.
   */
  def infallibleTransaction[A](body: InfallibleTransaction => Future[A]): Future[Either[TransactionFailure[Any], A]] =
    namedInfallibleTransaction(None)(body)

  def namedInfallibleTransaction[A](name: Option[String])(
    body: InfallibleTransaction => Future[A]
  ): Future[Either[TransactionFailure[Any], A]] = {
    val started = BigInt(Clock.now().toString)
    var state   = new Uint8Array(0)
    var attempt = BigInt(0)

    def loop(): Future[Either[TransactionFailure[Any], A]] = {
      val tx = new InfallibleTransaction
      invoke(body(tx)).flatMap(value => tx.checkActive().map(_ => value)).transformWith {
        case scala.util.Success(value) => Future.successful(Right(value))
        case scala.util.Failure(error) =>
          val cause   = tx.terminal.getOrElse(original(error))
          val failure = cause match {
            case OperationFailure(value) => value
            case other                   => other
          }
          tx.rollback().flatMap {
            case Some(compensation) =>
              Future.successful(Left(TransactionFailure.FailedAndRolledBackPartially(failure, compensation)))
            case None =>
              attempt += 1
              val kind = cause match {
                case _: OperationFailure     => "operation-error"
                case _: InterruptedException => "interrupted"
                case _                       => "exception"
              }
              val properties = List(
                "attempt"    -> JsPredicateValue.integer(js.BigInt(attempt.toString)),
                "error-kind" -> JsPredicateValue.text(kind)
              ) ++ name.toList.map(value => "transaction-name" -> JsPredicateValue.text(value))
              val decision = try {
                val id        = HostApi.getSelfMetadata().agentId
                val uuid      = id.componentId.uuid
                val component =
                  Uuid.toStandardString(Uuid(BigInt(uuid.highBits.toString), BigInt(uuid.lowBits.toString)))
                val noun   = s"urn:worker:$component/${id.agentId}"
                val policy = RetryApi.resolveRetryPolicy("saga", noun, properties)
                RetryApi.nextRetryDelay(
                  policy,
                  state,
                  (BigInt(Clock.now().toString) - started).max(0),
                  "saga",
                  noun,
                  properties
                )
              } catch {
                case NonFatal(e) => Left(e.getMessage)
              }
              decision match {
                case Left(reason) => Future.successful(Left(TransactionFailure.RetryEvaluationFailed(failure, reason)))
                case Right(next)  =>
                  state = next.state
                  next.delay match {
                    case None        => Future.successful(Left(TransactionFailure.RetryExhausted(failure)))
                    case Some(delay) =>
                      FutureInterop.fromPromise(Clock.waitFor(js.BigInt(delay.toString))).flatMap(_ => loop())
                  }
              }
          }
      }
    }
    loop()
  }

  final class InfallibleTransaction private[golem] () {
    private val tx                                     = new FallibleTransaction[Any]
    private[golem] var terminal: Option[Throwable]     = None
    private[golem] def checkActive(): Future[Unit]     = terminal.fold(Future.successful(()))(Future.failed)
    private[golem] def rollback(): Future[Option[Any]] = tx.rollback()

    def execute[In, Out, Err](operation: Operation[In, Out, Err], input: In): Future[Out] =
      checkActive().flatMap { _ =>
        invoke(operation.execute(input)).map {
          case Right(value) =>
            tx.register(() => invoke(operation.compensate(input, value)))
            value
          case Left(error) => throw OperationFailure(error)
        }.recoverWith { case error =>
          terminal = Some(original(error))
          Future.failed(error)
        }
      }
  }

  final class FallibleTransaction[Err] private[golem] () {
    private var compensations: List[() => Future[Either[Any, Unit]]]                 = Nil
    private[golem] def register(compensation: () => Future[Either[Any, Unit]]): Unit =
      compensations ::= compensation

    def execute[In, Out](operation: Operation[In, Out, Err], input: In): Future[Either[Err, Out]] =
      invoke(operation.execute(input)).map {
        case Right(value) =>
          register(() => invoke(operation.compensate(input, value)))
          Right(value)
        case left => left
      }

    private[golem] def rollback(): Future[Option[Any]] = {
      val pending = compensations
      compensations = Nil
      def run(remaining: List[() => Future[Either[Any, Unit]]]): Future[Option[Any]] = remaining match {
        case Nil          => Future.successful(None)
        case head :: tail =>
          invoke(head()).transformWith {
            case scala.util.Success(Right(_))    => run(tail)
            case scala.util.Success(Left(error)) => Future.successful(Some(error))
            case scala.util.Failure(error)       => Future.successful(Some(original(error)))
          }
      }
      run(pending)
    }

    def onFailure(error: Err): Future[TransactionFailure[Err]] =
      rollback().map {
        case None               => TransactionFailure.FailedAndRolledBackCompletely(error)
        case Some(compensation) =>
          TransactionFailure.FailedAndRolledBackPartially(error, compensation)
      }
  }

  @js.native
  @JSImport("wasi:clocks/monotonic-clock@0.3.0", JSImport.Namespace)
  private object Clock extends js.Object {
    def now(): js.BigInt                               = js.native
    def waitFor(duration: js.BigInt): js.Promise[Unit] = js.native
  }
}
