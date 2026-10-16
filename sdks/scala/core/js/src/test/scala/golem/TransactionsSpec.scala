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

import zio._
import zio.test._

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.typedarray.Uint8Array

object TransactionsSpec extends ZIOSpecDefault {
  import Transactions._

  private def f[A](a: A): Future[A] = Future.successful(a)

  private var restoreHost: () => Unit = () => ()

  private final class Host(delays: List[BigInt] = List(0, 0)) {
    private val previous = js.Dynamic.global.globalThis.selectDynamic("__golemScalaTestHost")
    restoreHost = () => js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(previous)
    var elapsed                                                                                              = BigInt(0)
    var calls                                                                                                = 0
    var contexts                                                                                             = List.empty[String]
    var tokens                                                                                               = List.empty[List[Int]]
    var waits                                                                                                = List.empty[BigInt]
    var evaluationError: Option[String]                                                                      = None
    var resolutionError: Option[String]                                                                      = None
    var resolved: js.Any                                                                                     = js.undefined
    private var context                                                                                      = ""
    private def describe(verb: String, noun: String, props: js.Array[js.Tuple2[String, js.Dynamic]]): String =
      verb + ":" + noun + ":" + props.map(p => p._1 + "=" + p._2.selectDynamic("val").toString).mkString(",")
    js.Dynamic.global.globalThis.__golemScalaTestHost = js.Dynamic.literal(
      now = (() => js.BigInt(elapsed.toString)): js.Function0[js.BigInt],
      waitFor = ((delay: js.BigInt) => {
        val n = BigInt(delay.toString)
        waits :+= n
        elapsed += n
        js.Promise.resolve[Unit](())
      }): js.Function1[js.BigInt, js.Promise[Unit]],
      getSelfMetadata = (
        () =>
          js.Dynamic.literal(
            status = "running",
            agentId = js.Dynamic.literal(
              agentId = "Order(1)",
              componentId =
                js.Dynamic.literal(uuid = js.Dynamic.literal(highBits = js.BigInt("0"), lowBits = js.BigInt("1")))
            )
          )
      ): js.Function0[js.Dynamic],
      resolveRetryPolicy = (
        (verb: String, noun: String, props: js.Array[js.Tuple2[String, js.Dynamic]]) => {
          context = describe(verb, noun, props)
          resolutionError.foreach(reason => throw js.JavaScriptException(reason))
          resolved
        }
      ): js.Function3[String, String, js.Array[js.Tuple2[String, js.Dynamic]], js.Any],
      nextRetryDelay = (
        (
          policy: js.Any,
          token: Uint8Array,
          duration: js.BigInt,
          verb: String,
          noun: String,
          props: js.Array[js.Tuple2[String, js.Dynamic]]
        ) => {
          require(policy == resolved)
          require(describe(verb, noun, props) == context)
          require(BigInt(duration.toString) == elapsed)
          contexts :+= context
          tokens :+= (0 until token.length).map(i => token(i).toInt).toList
          calls += 1
          evaluationError.foreach(reason => throw new IllegalArgumentException(reason))
          js.Dynamic.literal(
            state = new Uint8Array(js.Array(calls.toShort)),
            delay = delays.lift(calls - 1).map(n => js.BigInt(n.toString)).orUndefined
          )
        }
      ): js.Function6[js.Any, Uint8Array, js.BigInt, String, String, js.Array[
        js.Tuple2[String, js.Dynamic]
      ], js.Dynamic]
    )
  }

  def spec = suite("TransactionsSpec")(
    test("fallible synchronous body exceptions rethrow the original without retrying") {
      val cause = new RuntimeException("body defect", null, false, true) {}
      ZIO.fromFuture { _ =>
        fallibleTransaction[Int, String](_ => throw cause).failed
      }.map(error => assertTrue(error eq cause))
    },
    test("fallible thrown operation compensates successful operations in reverse and keeps its cause") {
      var order = List.empty[Int]
      val cause = new IllegalStateException("operation defect")
      val step  = operation[Int, Int, String](i => f(Right(i))) { (i, _) => order :+= i; f(Right(())) }
      val crash = operation[Unit, Int, String](_ => throw cause)((_, _) => f(Right(())))
      ZIO.fromFuture { _ =>
        fallibleTransaction[Int, String] { tx =>
          tx.execute(step, 1).flatMap(_ => tx.execute(step, 2)).flatMap(_ => tx.execute(crash, ()))
        }.failed
      }.map(error => assertTrue(error eq cause, order == List(2, 1)))
    },
    test("exhaustion retains returned failure and includes work and compensation time") {
      val host     = new Host(List(10))
      var attempts = 0
      val step     = operation[Unit, Int, String] { _ => host.elapsed += 3; f(Right(1)) } { (_, _) =>
        host.elapsed += 7; f(Right(()))
      }
      val fail = operation[Unit, Int, String](_ => f(Left("domain")))((_, _) => f(Right(())))
      ZIO.fromFuture { _ =>
        namedInfallibleTransaction(Some("checkout")) { tx =>
          attempts += 1
          tx.execute(step, ()).flatMap(_ => tx.execute(fail, ()))
        }
      }.map(result =>
        assertTrue(
          result == Left(TransactionFailure.RetryExhausted("domain")),
          attempts == 2,
          host.elapsed == 30,
          host.tokens == List(Nil, List(1)),
          host.contexts.head == "saga:urn:worker:00000000-0000-0000-0000-000000000001/Order(1):attempt=1,error-kind=operation-error,transaction-name=checkout",
          host.contexts.last.contains("attempt=2"),
          host.waits == List(BigInt(10))
        )
      )
    },
    test("caught exceptions use the host policy and preserve original cause on exhaustion") {
      val host  = new Host(Nil)
      val cause = new IllegalArgumentException("body")
      ZIO
        .fromFuture(_ => infallibleTransaction[Int](_ => throw cause))
        .map(result =>
          assertTrue(
            result == Left(TransactionFailure.RetryExhausted(cause)),
            host.calls == 1,
            host.contexts.head.contains("error-kind=exception")
          )
        )
    },
    test("interruption is compensated and passed to the retry evaluator") {
      val host        = new Host(Nil)
      val cause       = new InterruptedException("cancelled")
      var compensated = false
      val step        = operation[Unit, Int, String](_ => f(Right(1))) { (_, _) => compensated = true; f(Right(())) }
      ZIO.fromFuture { _ =>
        infallibleTransaction(tx => tx.execute(step, ()).flatMap(_ => Future.failed[Int](cause)))
      }.map(result =>
        assertTrue(
          result == Left(TransactionFailure.RetryExhausted(cause)),
          compensated,
          host.contexts.head.contains("error-kind=interrupted")
        )
      )
    },
    test("host resolver JavaScript errors preserve the original saga failure") {
      val host = new Host()
      host.resolutionError = Some("invalid resolver predicate")
      val original = new IllegalStateException("original")
      ZIO.fromFuture { _ =>
        infallibleTransaction[Int](_ => Future.failed(original))
      }.map {
        case Left(TransactionFailure.RetryEvaluationFailed(error: Throwable, reason)) =>
          assertTrue(
            error eq original,
            reason.contains("invalid resolver predicate"),
            host.calls == 0,
            host.waits.isEmpty
          )
        case _ => assertTrue(false)
      }
    },
    test("host evaluation errors are typed failures, not traps or fresh budgets") {
      val host = new Host()
      host.evaluationError = Some("unbounded saga policy")
      ZIO.fromFuture { _ =>
        infallibleTransaction[Int](_ => Future.failed(new IllegalStateException("original")))
      }.map {
        case Left(TransactionFailure.RetryEvaluationFailed(error: Throwable, reason)) =>
          assertTrue(
            error.getMessage == "original",
            reason == "unbounded saga policy",
            host.calls == 1,
            host.waits.isEmpty
          )
        case _ => assertTrue(false)
      }
    },
    test("policy changes do not reset the opaque continuation") {
      val host     = new Host(List(5))
      var attempts = 0
      ZIO.fromFuture { _ =>
        infallibleTransaction[Int] { _ =>
          attempts += 1
          if (attempts == 2) {
            host.resolved = js.Dynamic.literal(nodes = js.Array(js.Dynamic.literal(tag = "never")))
            host.evaluationError = Some("retry policy changed")
          }
          Future.failed(new IllegalStateException("original"))
        }
      }.map {
        case Left(TransactionFailure.RetryEvaluationFailed(_, reason)) =>
          assertTrue(
            reason == "retry policy changed",
            host.tokens == List(Nil, List(1)),
            attempts == 2,
            host.waits == List(BigInt(5))
          )
        case _ => assertTrue(false)
      }
    },
    test("infallible compensation interruption returns partial evidence without another attempt") {
      val host         = new Host()
      val interruption = new InterruptedException("undo")
      val step         = operation[Unit, Int, String](_ => f(Right(1)))((_, _) => throw interruption)
      val fail         = operation[Unit, Int, String](_ => f(Left("domain")))((_, _) => f(Right(())))
      ZIO.fromFuture { _ =>
        infallibleTransaction(tx => tx.execute(step, ()).flatMap(_ => tx.execute(fail, ())))
      }.map(result =>
        assertTrue(
          result == Left(TransactionFailure.FailedAndRolledBackPartially("domain", interruption)),
          host.calls == 0,
          host.waits.isEmpty
        )
      )
    },
    test("fallible partial rollback retains both failures when suppression is disabled") {
      val cause        = new RuntimeException("original", null, false, true) {}
      val interruption = new InterruptedException("undo interrupted")
      val step         = operation[Unit, Int, String](_ => f(Right(1)))((_, _) => throw interruption)
      ZIO.fromFuture { _ =>
        fallibleTransaction[Int, String](tx => tx.execute(step, ()).flatMap(_ => Future.failed(cause))).failed
      }.map(error =>
        assertTrue(
          error ne cause,
          error match {
            case PartialRollbackException(original, compensation: Throwable) =>
              (original eq cause) && (compensation eq interruption)
            case _ => false
          }
        )
      )
    },
    test("fallible body can handle a failed operation and continue") {
      var undos = 0
      val step  = operation[Boolean, Int, String](fail => f(if (fail) Left("handled") else Right(42))) { (_, _) =>
        undos += 1
        f(Right(()))
      }
      ZIO.fromFuture { _ =>
        fallibleTransaction[Int, String] { tx =>
          tx.execute(step, true).flatMap(_ => tx.execute(step, false))
        }
      }.map(result => assertTrue(result == Right(42), undos == 0))
    },
    test("swallowed execute failure retries and rejects follow-up operations") {
      val host     = new Host()
      var attempts = 0
      var runs     = 0
      var undos    = 0
      var rejected = 0
      val step     = operation[Unit, Int, String] { _ => runs += 1; f(Right(1)) } { (_, _) =>
        undos += 1
        f(Right(()))
      }
      val fail = operation[Unit, Int, String](_ => f(Left("retry")))((_, _) => f(Right(())))
      ZIO.fromFuture { _ =>
        infallibleTransaction { tx =>
          attempts += 1
          tx.execute(step, ()).flatMap { _ =>
            if (attempts == 1)
              tx.execute(fail, ())
                .recover { case _ => 0 }
                .flatMap { _ =>
                  tx.execute(step, ()).recover { case _ => rejected += 1; 0 }
                }
                .flatMap { _ =>
                  tx.execute(fail, ()).recover { case _ => rejected += 1; 0 }
                }
                .map(_ => "swallowed")
            else f("done")
          }
        }
      }.map(result =>
        assertTrue(result == Right("done"), attempts == 2, runs == 2, undos == 1, rejected == 2, host.calls == 1)
      )
    },
    test("swallowed compensation failure cannot succeed or retry") {
      val host     = new Host()
      var attempts = 0
      var runs     = 0
      var undos    = 0
      val step     = operation[Unit, Int, String] { _ => runs += 1; f(Right(1)) } { (_, _) =>
        undos += 1
        f(Left("broken compensation"))
      }
      val fail = operation[Unit, Int, String](_ => f(Left("retry")))((_, _) => f(Right(())))
      ZIO.fromFuture { _ =>
        infallibleTransaction { tx =>
          attempts += 1
          tx.execute(step, ())
            .flatMap(_ => tx.execute(fail, ()).recover { case _ => 0 })
            .flatMap(_ => tx.execute(step, ()).recover { case _ => 0 })
        }
      }.map(result =>
        assertTrue(
          result == Left(TransactionFailure.FailedAndRolledBackPartially("retry", "broken compensation")),
          attempts == 1,
          runs == 1,
          undos == 1,
          host.calls == 0
        )
      )
    },
    test("infallible saga compensates in reverse order and starts fresh attempts") {
      val host               = new Host(List(10, 20))
      var attempts           = 0
      var identity           = 0
      var events             = List.empty[String]
      def step(name: String) = operation[Unit, Int, String] { _ =>
        identity += 1
        events :+= s"run-$name-$identity"
        f(Right(identity))
      } { (_, output) =>
        events :+= s"undo-$name-$output"
        f(Right(()))
      }
      val finish =
        operation[Int, Int, String](input => f(if (attempts < 3) Left("retry") else Right(input)))((_, _) =>
          f(Right(()))
        )
      ZIO.fromFuture { _ =>
        infallibleTransaction { tx =>
          attempts += 1
          for {
            a      <- tx.execute(step("a"), ())
            b      <- tx.execute(step("b"), ())
            result <- tx.execute(finish, 100 * a + b)
          } yield result
        }
      }.map { result =>
        assertTrue(
          result == Right(506),
          host.waits == List(BigInt(10), BigInt(20)),
          host.tokens == List(Nil, List(1)),
          attempts == 3,
          events == List(
            "run-a-1",
            "run-b-2",
            "undo-b-2",
            "undo-a-1",
            "run-a-3",
            "run-b-4",
            "undo-b-4",
            "undo-a-3",
            "run-a-5",
            "run-b-6"
          )
        )
      }
    },
    test("fallible saga returns partial rollback without retrying") {
      var events = List.empty[Int]
      val op     = operation[Int, Int, String](i => f(Right(i))) { (i, _) =>
        events :+= i
        f(if (i == 2) Left("compensation") else Right(()))
      }
      ZIO.fromFuture { _ =>
        fallibleTransaction[Int, String] { tx =>
          for {
            _ <- tx.execute(op, 1)
            _ <- tx.execute(op, 2)
          } yield Left("failure")
        }
      }.map { result =>
        assertTrue(
          result == Left(TransactionFailure.FailedAndRolledBackPartially("failure", "compensation")),
          events == List(2)
        )
      }
    },
    test("Operation.apply creates executable operation") {
      val op = Operation[Int, String, String](
        run = i => f(Right(s"result-$i")),
        compensateFn = (_, _) => f(Right(()))
      )
      ZIO.fromFuture(_ => op.execute(42)).map(r => assertTrue(r == Right("result-42")))
    },
    test("Operation.execute returns Left on failure") {
      val op = Operation[Int, String, String](
        run = _ => f(Left("boom")),
        compensateFn = (_, _) => f(Right(()))
      )
      ZIO.fromFuture(_ => op.execute(1)).map(r => assertTrue(r == Left("boom")))
    },
    test("Operation.compensate returns Left on failure") {
      val op = Operation[Int, String, String](
        run = _ => f(Right("ok")),
        compensateFn = (_, _) => f(Left("comp-fail"))
      )
      ZIO.fromFuture(_ => op.compensate(1, "ok")).map(r => assertTrue(r == Left("comp-fail")))
    },
    test("Transactions.operation convenience creates Operation") {
      val op = Transactions.operation[Int, Int, String](in => f(Right(in * 2)))((_, _) => f(Right(())))
      ZIO.fromFuture(_ => op.execute(5)).map(r => assertTrue(r == Right(10)))
    },
    test("TransactionFailure.FailedAndRolledBackCompletely wraps error") {
      val failure: TransactionFailure[String] =
        TransactionFailure.FailedAndRolledBackCompletely("err")
      failure match {
        case TransactionFailure.FailedAndRolledBackCompletely(e) => assertTrue(e == "err")
        case _                                                   => assertTrue(false)
      }
    },
    test("TransactionFailure.FailedAndRolledBackPartially wraps both errors") {
      val failure: TransactionFailure[String] =
        TransactionFailure.FailedAndRolledBackPartially("err", "comp-err")
      failure match {
        case TransactionFailure.FailedAndRolledBackPartially(e, ce) =>
          assertTrue(e == "err", ce == "comp-err")
        case _ => assertTrue(false)
      }
    },
    test("TransactionFailure pattern match is exhaustive") {
      def describe[E](f: TransactionFailure[E]): String = f match {
        case TransactionFailure.FailedAndRolledBackCompletely(e)    => s"complete($e)"
        case TransactionFailure.FailedAndRolledBackPartially(e, ce) => s"partial($e, $ce)"
        case TransactionFailure.RetryExhausted(e)                   => s"exhausted($e)"
        case TransactionFailure.RetryEvaluationFailed(e, reason)    => s"evaluation($e, $reason)"
      }
      val f1 = TransactionFailure.FailedAndRolledBackCompletely("a")
      val f2 = TransactionFailure.FailedAndRolledBackPartially("a", "b")
      assertTrue(describe(f1) == "complete(a)", describe(f2) == "partial(a, b)")
    },
    test("FallibleTransaction.execute with all-successful operations") {
      val tx = new FallibleTransaction[String]
      val op = Operation[Int, Int, String](
        run = i => f(Right(i + 1)),
        compensateFn = (_, _) => f(Right(()))
      )
      ZIO.fromFuture { _ =>
        for {
          r1 <- tx.execute(op, 1)
          r2 <- tx.execute(op, 10)
        } yield (r1, r2)
      }.map { case (r1, r2) => assertTrue(r1 == Right(2), r2 == Right(11)) }
    },
    test("FallibleTransaction.execute propagates failure") {
      val tx     = new FallibleTransaction[String]
      val failOp = Operation[Int, Int, String](
        run = _ => f(Left("fail")),
        compensateFn = (_, _) => f(Right(()))
      )
      ZIO.fromFuture(_ => tx.execute(failOp, 1)).map(r => assertTrue(r == Left("fail")))
    },
    test("FallibleTransaction.onFailure runs compensations and returns complete rollback") {
      var compensated = false
      val tx          = new FallibleTransaction[String]
      val op          = Operation[Int, Int, String](
        run = i => f(Right(i)),
        compensateFn = (_, _) => { compensated = true; f(Right(())) }
      )
      ZIO.fromFuture { _ =>
        tx.execute(op, 1).flatMap(_ => tx.onFailure("err"))
      }.map { result =>
        assertTrue(compensated, result.isInstanceOf[TransactionFailure.FailedAndRolledBackCompletely[?]])
      }
    },
    test("FallibleTransaction.onFailure reports partial rollback on compensation failure") {
      val tx = new FallibleTransaction[String]
      val op = Operation[Int, Int, String](
        run = i => f(Right(i)),
        compensateFn = (_, _) => f(Left("comp-fail"))
      )
      ZIO.fromFuture { _ =>
        tx.execute(op, 1).flatMap(_ => tx.onFailure("err"))
      }.map {
        case TransactionFailure.FailedAndRolledBackPartially(e, ce) =>
          assertTrue(e == "err", ce == "comp-fail")
        case _ => assertTrue(false)
      }
    },
    test("FallibleTransaction.onFailure compensates in reverse order (synchronous)") {
      var order = List.empty[Int]
      val tx    = new FallibleTransaction[String]
      val op1   = Operation[Int, Int, String](
        run = i => f(Right(i)),
        compensateFn = (_, _) => { order = order :+ 1; f(Right(())) }
      )
      val op2 = Operation[Int, Int, String](
        run = i => f(Right(i)),
        compensateFn = (_, _) => { order = order :+ 2; f(Right(())) }
      )
      val op3 = Operation[Int, Int, String](
        run = i => f(Right(i)),
        compensateFn = (_, _) => { order = order :+ 3; f(Right(())) }
      )
      ZIO.fromFuture { _ =>
        for {
          _ <- tx.execute(op1, 10)
          _ <- tx.execute(op2, 20)
          _ <- tx.execute(op3, 30)
          r <- tx.onFailure("err")
        } yield r
      }.map { _ =>
        assertTrue(order == List(3, 2, 1))
      }
    },
    test("FallibleTransaction.onFailure compensates in reverse order (async)") {
      var order = List.empty[String]
      val tx    = new FallibleTransaction[String]

      def asyncOp(name: String, compName: String) = Operation[Unit, Unit, String](
        run = _ => Future(Right(())),
        compensateFn = (_, _) => Future { order = order :+ compName; Right(()) }
      )

      val reserve = asyncOp("reserve", "release")
      val charge  = asyncOp("charge", "refund")

      ZIO.fromFuture { _ =>
        for {
          _ <- tx.execute(reserve, ())
          _ <- tx.execute(charge, ())
          r <- tx.onFailure("err")
        } yield r
      }.map { _ =>
        assertTrue(order == List("refund", "release"))
      }
    },
    test("FallibleTransaction with no operations returns complete rollback") {
      val tx = new FallibleTransaction[String]
      ZIO.fromFuture(_ => tx.onFailure("err")).map { result =>
        assertTrue(result.isInstanceOf[TransactionFailure.FailedAndRolledBackCompletely[?]])
      }
    },
    test("Operation with Unit input") {
      val op = Operation[Unit, String, String](
        run = _ => f(Right("done")),
        compensateFn = (_, _) => f(Right(()))
      )
      ZIO.fromFuture(_ => op.execute(())).map(r => assertTrue(r == Right("done")))
    },
    test("Operation with complex error type") {
      final case class AppError(code: Int, message: String)
      val op = Operation[String, Int, AppError](
        run = _ => f(Left(AppError(404, "not found"))),
        compensateFn = (_, _) => f(Right(()))
      )
      ZIO.fromFuture(_ => op.execute("input")).map {
        case Left(AppError(code, msg)) => assertTrue(code == 404, msg == "not found")
        case _                         => assertTrue(false)
      }
    },
    test("Operation.compensate succeeds") {
      val op = Operation[Int, String, String](
        run = _ => f(Right("ok")),
        compensateFn = (_, _) => f(Right(()))
      )
      ZIO.fromFuture(_ => op.compensate(42, "ok")).map(r => assertTrue(r == Right(())))
    }
  ) @@ TestAspect.after(ZIO.succeed {
    restoreHost()
    restoreHost = () => ()
  }) @@ TestAspect.sequential
}
