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

package golem.tool

import golem.schema.{IntoSchema, TypedSchemaValue}
import zio.ZIO
import zio.test.*

import scala.collection.mutable
import scala.concurrent.{Future, Promise}

object ToolMiddlewareOwnershipSpec extends ZIOSpecDefault {
  private type Outcome = Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]

  private val unitInput = ToolErrorSupport.unitPayload
  private val empty     = ToolMiddlewareResult(None, None, None)

  private final class ClosableInput extends ToolMiddlewareInputHandle {
    var closeCount = 0

    override private[golem] def close(): Future[Unit] = {
      closeCount += 1
      Future.successful(())
    }
  }

  private final class ClosableOutput(
    failClose: Boolean = false,
    drain: Future[Unit] = Future.successful(())
  ) extends ToolMiddlewareOutputHandle {
    var closeCount = 0

    override private[golem] def close(): Future[Unit] = {
      closeCount += 1
      if (failClose) Future.failed(new RuntimeException("close failed"))
      else Future.successful(())
    }

    override private[golem] def drained: Future[Unit] = drain
  }

  private final class FunctionRaw(
    run: (List[String], TypedSchemaValue, Option[ToolMiddlewareInputHandle]) => Future[Outcome]
  ) extends RawToolUnderlying {
    val calls = mutable.ListBuffer.empty[(List[String], TypedSchemaValue, Option[ToolMiddlewareInputHandle])]

    def invoke(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolMiddlewareInputHandle]
    ): Future[Outcome] = {
      calls += ((commandPath, input, stdin))
      run(commandPath, input, stdin)
    }
  }

  private def rawSuccess(result: ToolMiddlewareResult = empty): FunctionRaw =
    new FunctionRaw((_, _, _) => Future.successful(Right(result)))

  private def withUnderlying(
    raw: RawToolUnderlying,
    stdin: Option[ToolMiddlewareInputHandle] = None
  )(
    invoke: RawToolUnderlying => Future[Outcome]
  ): Future[Outcome] =
    ToolMiddlewareOwnershipRuntime.withInvocationScopedUnderlying(raw, stdin)(invoke)

  private def sequential(underlying: RawToolUnderlying, remaining: Int): Future[Outcome] =
    if (remaining == 0) Future.successful(Right(empty))
    else
      underlying
        .invoke(List("run"), unitInput, None)
        .flatMap(_ => sequential(underlying, remaining - 1))(ToolInvokerRuntime.executionContext)

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolMiddlewareOwnershipSpec")(
      test("underlying observers start one get lazily and retain its terminal") {
        var gets      = 0
        val terminal  = Promise[Either[ToolUnderlyingError[Nothing], String]]()
        val admission = ToolUnderlyingAdmission(None, None, () => { gets += 1; terminal.future }, () => (), () => ())
        val before    = gets
        val first     = admission.result
        val second    = admission.result
        terminal.success(Right("terminal"))
        for {
          a <- ZIO.fromFuture(_ => first)
          b <- ZIO.fromFuture(_ => second)
          c <- ZIO.fromFuture(_ => admission.result)
        } yield assertTrue(before == 0, gets == 1, first eq second, a == Right("terminal"), b == a, c == a)
      },
      test("admission and await views share one child in both orders through completion mapping") {
        ZIO
          .foreach(for {
            awaitFirst <- List(false, true)
            mapped     <- List(false, true)
          } yield (awaitFirst, mapped)) { case (awaitFirst, mapped) =>
            var admissions = 0
            var gets       = 0
            var cancels    = 0
            val admission  = ToolUnderlyingAdmission[Nothing, ToolMiddlewareResult](
              None,
              None,
              () => { gets += 1; Future.successful(Right(empty)) },
              () => cancels += 1,
              () => ()
            )
            admissions += 1
            val base = ToolUnderlyingInvocation(Future.successful(admission))
            val call =
              if mapped then
                ToolUnderlyingRuntime
                  .complete(base)(_ => Right("mapped"))
                  .asInstanceOf[ToolUnderlyingInvocation[Nothing, Any]]
              else base.asInstanceOf[ToolUnderlyingInvocation[Nothing, Any]]

            val observed =
              if awaitFirst then {
                val awaited = call.toMiddlewareResult
                call.admission
                  .map(_.cancel())(ToolInvokerRuntime.executionContext)
                  .flatMap(_ => awaited)(
                    ToolInvokerRuntime.executionContext
                  )
              } else
                call.admission.flatMap { admitted =>
                  admitted.cancel()
                  call.toMiddlewareResult
                }(ToolInvokerRuntime.executionContext)

            ZIO
              .fromFuture(_ => observed)
              .map(result =>
                assertTrue(
                  admissions == 1,
                  gets == 1,
                  cancels == 1,
                  result == Right(if mapped then "mapped" else empty)
                )
              )
          }
          .map(results => results.reduce(_ && _))
      },
      test("ignored mapped typed invocation remains unobserved and drops without get") {
        var gets  = 0
        var drops = 0
        val raw   = new RawToolUnderlying {
          def invoke(
            commandPath: List[String],
            input: TypedSchemaValue,
            stdin: Option[ToolMiddlewareInputHandle]
          ): Future[Outcome] = Future.failed(new IllegalStateException("unexpected convenience invoke"))

          override def start(
            commandPath: List[String],
            input: TypedSchemaValue,
            stdin: Option[ToolMiddlewareInputHandle]
          ): ToolUnderlyingInvocation[TypedSchemaValue, ToolMiddlewareResult] =
            ToolUnderlyingInvocation(
              Future.successful(
                ToolUnderlyingAdmission(
                  None,
                  None,
                  () => {
                    gets += 1; Promise[Either[ToolUnderlyingError[TypedSchemaValue], ToolMiddlewareResult]]().future
                  },
                  () => (),
                  () => drops += 1
                )
              )
            )
        }
        val result = withUnderlying(raw) { underlying =>
          ToolUnderlyingRuntime.complete(underlying.start(Nil, unitInput, None))(_ => Right("mapped"))
          Future.successful(Right(empty))
        }
        ZIO.fromFuture(_ => result).map(outcome => assertTrue(outcome == Right(empty), gets == 0, drops == 1))
      },
      test("observed mapped typed invocation retains result and both drains before drop") {
        var gets        = 0
        var drops       = 0
        val getStarted  = Promise[Unit]()
        val terminal    = Promise[Either[ToolUnderlyingError[TypedSchemaValue], ToolMiddlewareResult]]()
        val stdoutDrain = Promise[Unit]()
        val stderrDrain = Promise[Unit]()
        val stdout      = new ClosableOutput(drain = stdoutDrain.future)
        val stderr      = new ClosableOutput(drain = stderrDrain.future)
        val raw         = new RawToolUnderlying {
          def invoke(
            commandPath: List[String],
            input: TypedSchemaValue,
            stdin: Option[ToolMiddlewareInputHandle]
          ): Future[Outcome] = Future.failed(new IllegalStateException("unexpected convenience invoke"))

          override def start(
            commandPath: List[String],
            input: TypedSchemaValue,
            stdin: Option[ToolMiddlewareInputHandle]
          ): ToolUnderlyingInvocation[TypedSchemaValue, ToolMiddlewareResult] =
            ToolUnderlyingInvocation(
              Future.successful(
                ToolUnderlyingAdmission(
                  Some(stdout),
                  Some(stderr),
                  () => { gets += 1; getStarted.trySuccess(()); terminal.future },
                  () => (),
                  () => drops += 1
                )
              )
            )
        }
        val result = withUnderlying(raw) { underlying =>
          val mapped = ToolUnderlyingRuntime.complete(underlying.start(Nil, unitInput, None))(_ => Right("mapped"))
          mapped.toMiddlewareResult
          Future.successful(Right(empty))
        }
        for {
          _            <- ZIO.fromFuture(_ => getStarted.future)
          pendingResult = !result.isCompleted && gets == 1 && drops == 0
          _             = terminal.success(Right(ToolMiddlewareResult(None, Some(stdout), Some(stderr))))
          pendingDrains = !result.isCompleted && drops == 0
          _             = stdoutDrain.success(())
          pendingStderr = !result.isCompleted && drops == 0
          _             = stderrDrain.success(())
          outcome      <- ZIO.fromFuture(_ => result)
        } yield assertTrue(pendingResult, pendingDrains, pendingStderr, outcome == Right(empty), gets == 1, drops == 1)
      },
      test("underlying cancellation and resource exhaustion remain distinct until wire encoding") {
        ZIO
          .foreach(
            List(
              (ToolUnderlyingError.Cancelled, ToolInvokeError.Cancelled),
              (ToolUnderlyingError.ResourceExhausted("quota"), ToolInvokeError.ResourceExhausted("quota"))
            )
          ) { case (underlyingError, expected) =>
            val admission = ToolUnderlyingAdmission[Nothing, ToolMiddlewareResult](
              None,
              None,
              () => Future.successful(Left(underlyingError)),
              () => (),
              () => ()
            )
            val raw = new RawToolUnderlying {
              def invoke(path: List[String], input: TypedSchemaValue, stdin: Option[ToolMiddlewareInputHandle])
                : Future[Outcome] =
                ToolUnderlyingInvocation(Future.successful(admission)).toMiddlewareResult
              override def start(
                path: List[String],
                input: TypedSchemaValue,
                stdin: Option[ToolMiddlewareInputHandle]
              ) =
                ToolUnderlyingInvocation(Future.successful(admission))
            }
            for {
              observed <- ZIO.fromFuture(_ => raw.invoke(Nil, unitInput, None))
              tracked  <- ZIO.fromFuture(_ => withUnderlying(raw)(_.invoke(Nil, unitInput, None)))
            } yield assertTrue(
              observed == Left(expected),
              tracked == observed,
              ToolInvokeError.toWire(expected).isInstanceOf[golem.tool.wire.WitToolError.ConstraintViolation]
            )
          }
          .map(results => results.reduce(_ && _))
      },
      test("allows zero, one, and multiple sequential underlying calls") {
        ZIO
          .foreach(List(0, 1, 3)) { count =>
            val raw = rawSuccess()
            ZIO
              .fromFuture(_ => withUnderlying(raw)(sequential(_, count)))
              .map(result => (count, raw.calls.size, result))
          }
          .map(results =>
            assertTrue(results.forall { case (expected, actual, result) =>
              expected == actual && result == Right(empty)
            })
          )
      },
      test("allows overlapping calls to complete in reverse order") {
        val firstResponse  = Promise[Outcome]()
        val secondResponse = Promise[Outcome]()
        val raw            =
          new FunctionRaw((path, _, _) => if (path == List("first")) firstResponse.future else secondResponse.future)
        val result = withUnderlying(raw) { underlying =>
          val first  = underlying.invoke(List("first"), unitInput, None)
          val second = underlying.invoke(List("second"), unitInput, None)
          second.flatMap { _ =>
            firstResponse.success(Right(empty))
            first.map(_ => Right(empty))(ToolInvokerRuntime.executionContext)
          }(ToolInvokerRuntime.executionContext)
        }
        val bothAdmitted = raw.calls.size == 2
        secondResponse.success(Right(empty))
        ZIO
          .fromFuture(_ => result)
          .map(outcome =>
            assertTrue(
              outcome == Right(empty),
              bothAdmitted,
              raw.calls.map(_._1).toList == List(List("first"), List("second"))
            )
          )
      },
      test("revokes escaped wrappers after the middleware settles") {
        val raw                        = rawSuccess()
        var escaped: RawToolUnderlying = null
        for {
          _ <- ZIO.fromFuture(_ =>
                 withUnderlying(raw) { underlying =>
                   escaped = underlying
                   Future.successful(Right(empty))
                 }
               )
          failure <- ZIO.fromFuture(_ => escaped.invoke(Nil, unitInput, None)).flip
        } yield assertTrue(
          failure.isInstanceOf[ToolUnderlyingMisuseException],
          failure.asInstanceOf[ToolUnderlyingMisuseException].reason == ToolUnderlyingMisuse.Revoked,
          raw.calls.isEmpty
        )
      },
      test("revocation retains an observed convenience call through its terminal") {
        val response              = Promise[Outcome]()
        val raw                   = new FunctionRaw((_, _, _) => response.future)
        var call: Future[Outcome] = null
        val result                = withUnderlying(raw) { underlying =>
          call = underlying.invoke(Nil, unitInput, None)
          Future.successful(Right(empty))
        }
        val pending = !call.isCompleted && !result.isCompleted && raw.calls.size == 1
        response.success(Right(empty))
        ZIO
          .fromFuture(_ => result)
          .map(outcome => assertTrue(pending, call.isCompleted, outcome == Right(empty)))
      },
      test("closes unforwarded stdin on short-circuit and failed callback paths") {
        val shortInput  = new ClosableInput
        val failedInput = new ClosableInput
        val failure     = new RuntimeException("middleware failed")
        for {
          short <-
            ZIO.fromFuture(_ => withUnderlying(rawSuccess(), Some(shortInput))(_ => Future.successful(Right(empty))))
          failed <- ZIO
                      .fromFuture(_ => withUnderlying(rawSuccess(), Some(failedInput))(_ => Future.failed(failure)))
                      .flip
        } yield assertTrue(
          short == Right(empty),
          failed eq failure,
          shortInput.closeCount == 1,
          failedInput.closeCount == 1
        )
      },
      test("transfers stdin once and permits a retry without stdin") {
        val stdin                                = new ClosableInput
        val raw                                  = rawSuccess()
        var reason: Option[ToolUnderlyingMisuse] = None
        val result                               = withUnderlying(raw, Some(stdin)) { underlying =>
          underlying
            .invoke(List("first"), unitInput, Some(stdin))
            .flatMap(_ =>
              underlying
                .invoke(List("reuse"), unitInput, Some(stdin))
                .recover { case error: ToolUnderlyingMisuseException =>
                  reason = Some(error.reason)
                  Left(ToolInvokeError.InvalidResult("reuse rejected"))
                }(ToolInvokerRuntime.executionContext)
            )(ToolInvokerRuntime.executionContext)
            .flatMap(_ => underlying.invoke(List("retry"), unitInput, None))(ToolInvokerRuntime.executionContext)
        }
        ZIO
          .fromFuture(_ => result)
          .map(outcome =>
            assertTrue(
              outcome == Right(empty),
              reason.contains(ToolUnderlyingMisuse.StreamAlreadyTransferred),
              raw.calls.map(_._1).toList == List(List("first"), List("retry")),
              stdin.closeCount == 0
            )
          )
      },
      test("forwards selected stdout and closes abandoned stdout exactly once") {
        val selected  = new ClosableOutput
        val abandoned = new ClosableOutput
        val responses = mutable.Queue[Outcome](
          Right(ToolMiddlewareResult(None, Some(selected), None)),
          Right(ToolMiddlewareResult(None, Some(abandoned), None))
        )
        val raw = new FunctionRaw((_, _, _) => Future.successful(responses.dequeue()))
        for {
          result <- ZIO.fromFuture(_ =>
                      withUnderlying(raw) { underlying =>
                        underlying
                          .invoke(List("selected"), unitInput, None)
                          .flatMap { selectedResult =>
                            underlying
                              .invoke(List("abandoned"), unitInput, None)
                              .map(_ => selectedResult)(ToolInvokerRuntime.executionContext)
                          }(ToolInvokerRuntime.executionContext)
                      }
                    )
          _ <- ZIO.fromFuture(_ => result.toOption.flatMap(_.stdout).get.close())
        } yield assertTrue(
          result.toOption.flatMap(_.stdout).contains(selected),
          selected.closeCount == 1,
          abandoned.closeCount == 1
        )
      },
      test("tracks fresh final stdout before a later callback failure") {
        val stdout         = new ClosableOutput
        val failure        = new RuntimeException("encoding failed")
        val failingEncoder = new IntoSchema[String] {
          val graph = IntoSchema[String].graph

          def toValue(value: String) = throw failure
        }
        val result = withUnderlying(rawSuccess()) { underlying =>
          Future.successful(
            ToolMiddlewareInvokerRuntime.encodeValueStdout("value", stdout, failingEncoder, underlying)
          )
        }
        ZIO
          .fromFuture(_ => result)
          .flip
          .map(error => assertTrue(error eq failure, stdout.closeCount == 1))
      },
      test("tracks repeated stdout identity once when abandoned or selected") {
        val abandoned    = new ClosableOutput
        val selected     = new ClosableOutput
        val abandonedRaw =
          new FunctionRaw((_, _, _) => Future.successful(Right(ToolMiddlewareResult(None, Some(abandoned), None))))
        val selectedRaw =
          new FunctionRaw((_, _, _) => Future.successful(Right(ToolMiddlewareResult(None, Some(selected), None))))
        for {
          _ <- ZIO.fromFuture(_ =>
                 withUnderlying(abandonedRaw) { underlying =>
                   underlying
                     .invoke(Nil, unitInput, None)
                     .flatMap(_ => underlying.invoke(Nil, unitInput, None))(ToolInvokerRuntime.executionContext)
                     .map(_ => Right(empty))(ToolInvokerRuntime.executionContext)
                 }
               )
          result <- ZIO.fromFuture(_ =>
                      withUnderlying(selectedRaw) { underlying =>
                        underlying
                          .invoke(Nil, unitInput, None)
                          .flatMap(_ => underlying.invoke(Nil, unitInput, None))(ToolInvokerRuntime.executionContext)
                      }
                    )
        } yield assertTrue(
          abandoned.closeCount == 1,
          result.toOption.flatMap(_.stdout).contains(selected),
          selected.closeCount == 0
        )
      },
      test("closes stdout after malformed underlying results and callback failures") {
        val malformedOutput = new ClosableOutput
        val failedOutput    = new ClosableOutput
        val malformed       = IntoSchema[Boolean].toTyped(true).copy(value = IntoSchema[String].toValue("wrong"))
        val malformedRaw    = rawSuccess(ToolMiddlewareResult(Some(malformed), Some(malformedOutput), None))
        val failedRaw       = rawSuccess(ToolMiddlewareResult(None, Some(failedOutput), None))
        val failure         = new RuntimeException("handler failed")
        for {
          invalid <- ZIO.fromFuture(_ => withUnderlying(malformedRaw)(_.invoke(Nil, unitInput, None)))
          failed  <- ZIO
                      .fromFuture(_ =>
                        withUnderlying(failedRaw) { underlying =>
                          underlying
                            .invoke(Nil, unitInput, None)
                            .flatMap(_ => Future.failed(failure))(ToolInvokerRuntime.executionContext)
                        }
                      )
                      .flip
        } yield assertTrue(
          invalid.left.exists(_.isInstanceOf[ToolInvokeError.InvalidResult]),
          failed eq failure,
          malformedOutput.closeCount == 1,
          failedOutput.closeCount == 1
        )
      },
      test("best-effort cleanup continues after a stream close failure") {
        val first     = new ClosableOutput(failClose = true)
        val second    = new ClosableOutput
        val responses = mutable.Queue[Outcome](
          Right(ToolMiddlewareResult(None, Some(first), None)),
          Right(ToolMiddlewareResult(None, Some(second), None))
        )
        val raw = new FunctionRaw((_, _, _) => Future.successful(responses.dequeue()))
        ZIO
          .fromFuture(_ =>
            withUnderlying(raw) { underlying =>
              underlying
                .invoke(List("first"), unitInput, None)
                .flatMap(_ => underlying.invoke(List("second"), unitInput, None))(ToolInvokerRuntime.executionContext)
                .map(_ => Right(empty))(ToolInvokerRuntime.executionContext)
            }
          )
          .map(outcome => assertTrue(outcome == Right(empty), first.closeCount == 1, second.closeCount == 1))
      }
    ) @@ TestAspect.sequential
}
