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

import zio._
import zio.test._

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

object TransactionsSpec extends ZIOSpecDefault {
  import Transactions._

  private def f[A](a: A): Future[A] = Future.successful(a)

  def spec = suite("TransactionsSpec")(
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
