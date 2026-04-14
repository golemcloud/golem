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

package example.integrationtests

import golem.Transactions
import golem.Transactions._
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.ExecutionContext.Implicits.global
import scala.concurrent.Future

@agentImplementation()
final class TransactionsDemoImpl(@unused private val name: String) extends TransactionsDemo {

  private var trace: List[String] = Nil

  private def appendTrace(msg: String): Unit =
    trace = trace :+ msg

  private def resetAndGetTrace(): String = {
    val result = trace.mkString("\n")
    trace = Nil
    result
  }

  override def infallibleDemo(): Future[String] = {
    val sb = new StringBuilder
    sb.append("=== Infallible Transaction Demo ===\n")

    val op1 = Transactions.operation[Int, Int, String](
      run = i => { appendTrace(s"op1.run($i)"); Future.successful(Right(i + 10)) }
    )(
      compensate = (input, output) => { appendTrace(s"op1.compensate($input,$output)"); Future.successful(Right(())) }
    )
    val op2 = Transactions.operation[Int, Int, String](
      run = i => { appendTrace(s"op2.run($i)"); Future.successful(Right(i * 2)) }
    )(
      compensate = (input, output) => { appendTrace(s"op2.compensate($input,$output)"); Future.successful(Right(())) }
    )

    Transactions.infallibleTransaction { tx =>
      tx.execute(op1, 5).flatMap { r1 =>
        appendTrace(s"op1 result=$r1")
        tx.execute(op2, r1).map { r2 =>
          appendTrace(s"op2 result=$r2")
          sb.append(s"transaction result=$r2\n")
          sb.append("trace:\n")
          sb.append(resetAndGetTrace())
          sb.append("\n")
          sb.result()
        }
      }
    }
  }

  override def fallibleSuccessDemo(): Future[String] = {
    val sb = new StringBuilder
    sb.append("=== Fallible Transaction (Success) Demo ===\n")

    val op1 = Operation[Int, Int, String](
      run = i => { appendTrace(s"op1.run($i)"); Future.successful(Right(i + 100)) },
      compensateFn = (input, output) => { appendTrace(s"op1.compensate($input,$output)"); Future.successful(Right(())) }
    )
    val op2 = Operation[Int, Int, String](
      run = i => { appendTrace(s"op2.run($i)"); Future.successful(Right(i - 50)) },
      compensateFn = (input, output) => { appendTrace(s"op2.compensate($input,$output)"); Future.successful(Right(())) }
    )

    Transactions.fallibleTransaction[Int, String] { tx =>
      tx.execute(op1, 1).flatMap {
        case Left(err) =>
          appendTrace(s"op1 result=Left($err)")
          Future.successful(Left(err))
        case Right(v1) =>
          appendTrace(s"op1 result=Right($v1)")
          tx.execute(op2, v1).map { r2 =>
            appendTrace(s"op2 result=$r2")
            r2
          }
      }
    }.map { result =>
      sb.append(s"transaction result=$result\n")
      sb.append("trace:\n")
      sb.append(resetAndGetTrace())
      sb.append("\n")
      sb.result()
    }
  }

  override def fallibleFailureDemo(): Future[String] = {
    val sb = new StringBuilder
    sb.append("=== Fallible Transaction (Failure + Rollback) Demo ===\n")

    val op1 = Operation[Int, Int, String](
      run = i => { appendTrace(s"op1.run($i)"); Future.successful(Right(i + 1)) },
      compensateFn = (input, output) => { appendTrace(s"op1.compensate($input,$output)"); Future.successful(Right(())) }
    )
    val op2 = Operation[Int, Int, String](
      run = i => { appendTrace(s"op2.run($i)"); Future.successful(Right(i + 2)) },
      compensateFn = (input, output) => { appendTrace(s"op2.compensate($input,$output)"); Future.successful(Right(())) }
    )
    val failOp = Operation[Int, Int, String](
      run = i => { appendTrace(s"failOp.run($i) -> LEFT"); Future.successful(Left("intentional-failure")) },
      compensateFn = (input, output) => { appendTrace(s"failOp.compensate($input,$output)"); Future.successful(Right(())) }
    )

    Transactions.fallibleTransaction[Int, String] { tx =>
      tx.execute(op1, 10).flatMap {
        case Left(err) =>
          appendTrace(s"op1 result=Left($err)")
          Future.successful(Left(err))
        case Right(v1) =>
          appendTrace(s"op1 result=Right($v1)")
          tx.execute(op2, v1).flatMap {
            case Left(err) =>
              appendTrace(s"op2 result=Left($err)")
              Future.successful(Left(err))
            case Right(v2) =>
              appendTrace(s"op2 result=Right($v2)")
              tx.execute(failOp, v2).map { r3 =>
                appendTrace(s"failOp result=$r3")
                r3
              }
          }
      }
    }.map { result =>
      val resultStr = result match {
        case Right(v)                                                            => s"Right($v)"
        case Left(TransactionFailure.FailedAndRolledBackCompletely(err))         => s"FailedAndRolledBackCompletely($err)"
        case Left(TransactionFailure.FailedAndRolledBackPartially(err, compErr)) =>
          s"FailedAndRolledBackPartially($err, $compErr)"
      }

      sb.append(s"transaction result=$resultStr\n")
      sb.append("trace:\n")
      sb.append(resetAndGetTrace())
      sb.append("\n")
      sb.result()
    }
  }
}
