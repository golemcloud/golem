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

object TransactionsSpec extends ZIOSpecDefault {
  import Transactions._

  private def f[A](a: A): Future[A] = Future.successful(a)

  def spec = suite("TransactionsSpec")(
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
      }.map(result => assertTrue(result == "done", attempts == 2, runs == 2, undos == 1, rejected == 2))
    },
    test("swallowed compensation failure cannot succeed or retry") {
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
        }.map(_ => false).recover { case _ => true }
      }.map(failed => assertTrue(failed, attempts == 1, runs == 1, undos == 1))
    },
    test("infallible saga compensates in reverse order and starts fresh attempts") {
      var attempts           = 0
      var events             = List.empty[String]
      def step(name: String) = operation[Unit, Unit, String] { _ =>
        events :+= s"run-$name-$attempts"
        f(Right(()))
      } { (_, _) =>
        events :+= s"undo-$name-$attempts"
        f(Right(()))
      }
      val finish =
        operation[Unit, Int, String](_ => f(if (attempts < 3) Left("retry") else Right(42)))((_, _) => f(Right(())))
      ZIO.fromFuture { _ =>
        infallibleTransaction { tx =>
          attempts += 1
          for {
            _      <- tx.execute(step("a"), ())
            _      <- tx.execute(step("b"), ())
            result <- tx.execute(finish, ())
          } yield result
        }
      }.map { result =>
        assertTrue(
          result == 42,
          attempts == 3,
          events == List(
            "run-a-1",
            "run-b-1",
            "undo-b-1",
            "undo-a-1",
            "run-a-2",
            "run-b-2",
            "undo-b-2",
            "undo-a-2",
            "run-a-3",
            "run-b-3"
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
  )
}
