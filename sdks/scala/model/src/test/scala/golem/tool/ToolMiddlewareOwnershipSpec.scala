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
  private type Outcome = Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]

  private val unitInput = ToolErrorSupport.unitPayload
  private val empty     = ToolInvokeResult(None, None)

  private final class ClosableInput extends ToolInputStream {
    var closeCount = 0

    override private[golem] def close(): Future[Unit] = {
      closeCount += 1
      Future.successful(())
    }
  }

  private final class ClosableOutput(failClose: Boolean = false) extends ToolOutputStream {
    var closeCount = 0

    override private[golem] def close(): Future[Unit] = {
      closeCount += 1
      if (failClose) Future.failed(new RuntimeException("close failed"))
      else Future.successful(())
    }
  }

  private final class FunctionRaw(run: (List[String], TypedSchemaValue, Option[ToolInputStream]) => Future[Outcome])
      extends RawToolUnderlying {
    val calls = mutable.ListBuffer.empty[(List[String], TypedSchemaValue, Option[ToolInputStream])]

    def invoke(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolInputStream]
    ): Future[Outcome] = {
      calls += ((commandPath, input, stdin))
      run(commandPath, input, stdin)
    }
  }

  private def rawSuccess(result: ToolInvokeResult = empty): FunctionRaw =
    new FunctionRaw((_, _, _) => Future.successful(Right(result)))

  private def withUnderlying(
    raw: RawToolUnderlying,
    stdin: Option[ToolInputStream] = None
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
      test("rejects overlapping calls as SDK misuse") {
        val response                             = Promise[Outcome]()
        val raw                                  = new FunctionRaw((_, _, _) => response.future)
        var reason: Option[ToolUnderlyingMisuse] = None
        val result                               = withUnderlying(raw) { underlying =>
          val first = underlying.invoke(List("first"), unitInput, None)
          underlying
            .invoke(List("overlap"), unitInput, None)
            .recover { case error: ToolUnderlyingMisuseException =>
              reason = Some(error.reason)
              Left(ToolInvokeError.InvalidResult("overlap rejected"))
            }(ToolInvokerRuntime.executionContext)
            .flatMap { _ =>
              response.success(Right(empty))
              first.map(_ => Right(empty))(ToolInvokerRuntime.executionContext)
            }(ToolInvokerRuntime.executionContext)
        }
        ZIO
          .fromFuture(_ => result)
          .map(outcome =>
            assertTrue(
              outcome == Right(empty),
              reason.contains(ToolUnderlyingMisuse.OverlappingInvocation),
              raw.calls.size == 1
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
      test("revocation waits for an already-started invocation") {
        val response              = Promise[Outcome]()
        val raw                   = new FunctionRaw((_, _, _) => response.future)
        var call: Future[Outcome] = null
        val result                = withUnderlying(raw) { underlying =>
          call = underlying.invoke(Nil, unitInput, None)
          Future.successful(Right(empty))
        }
        val pending = result.value.isEmpty
        response.success(Right(empty))
        ZIO
          .fromFuture(_ => result)
          .map(outcome => assertTrue(pending, call.isCompleted, outcome == Right(empty), raw.calls.size == 1))
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
          Right(ToolInvokeResult(None, Some(selected))),
          Right(ToolInvokeResult(None, Some(abandoned)))
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
          new FunctionRaw((_, _, _) => Future.successful(Right(ToolInvokeResult(None, Some(abandoned)))))
        val selectedRaw = new FunctionRaw((_, _, _) => Future.successful(Right(ToolInvokeResult(None, Some(selected)))))
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
        val malformedRaw    = rawSuccess(ToolInvokeResult(Some(malformed), Some(malformedOutput)))
        val failedRaw       = rawSuccess(ToolInvokeResult(None, Some(failedOutput)))
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
          Right(ToolInvokeResult(None, Some(first))),
          Right(ToolInvokeResult(None, Some(second)))
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
