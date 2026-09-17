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

package golem.host

import golem.host.js.{JsPolicyNode, JsRetryPolicyTree}
import zio.{Exit, Task, ZIO}
import zio.test._

import scala.collection.mutable.ListBuffer
import scala.concurrent.Future
import scala.concurrent.duration._

object LocalRetrySpec extends ZIOSpecDefault {
  private final class TestRuntime(randomValue: Double = 0.0) extends LocalRetry.Runtime {
    var now: Long                          = 0L
    val sleeps: ListBuffer[FiniteDuration] = ListBuffer.empty

    override def nowNanos(): Long = now

    override def sleep(delay: FiniteDuration): Future[Unit] = {
      sleeps += delay
      now += delay.toNanos
      Future.successful(())
    }

    override def random(): Double = randomValue
  }

  private final case class AttemptError(number: Int, status: Int) extends RuntimeException

  private def run(
    policy: Retry.Policy,
    failures: List[AttemptError],
    runtime: TestRuntime = new TestRuntime(),
    properties: Throwable => Iterable[Retry.Property] = {
      case error: AttemptError => List(Retry.Props.statusCode -> error.status)
      case _                   => Nil
    }
  ): Task[(Int, List[FiniteDuration])] = {
    var attempts = 0
    ZIO
      .fromFuture(_ =>
        LocalRetry.retryWith(
          policy,
          properties,
          () => {
            attempts += 1
            failures.lift(attempts - 1) match {
              case Some(error) => Future.failed(error)
              case None        => Future.successful(attempts)
            }
          },
          runtime
        )
      )
      .map(result => result -> runtime.sleeps.toList)
  }

  def spec = suite("LocalRetrySpec")(
    test("retries until eventual success with exponential delays") {
      val runtime = new TestRuntime()
      run(
        Retry.Policy.exponential(2.seconds, 3.0).maxRetries(4),
        List(AttemptError(1, 500), AttemptError(2, 500)),
        runtime
      ).map { case (attempts, delays) =>
        assertTrue(attempts == 3, delays == List(2.seconds, 6.seconds))
      }
    },
    test("count box permits exactly maxRetries retries and preserves the final failure") {
      val finalError = AttemptError(3, 500)
      var attempts   = 0
      ZIO
        .fromFuture(_ =>
          Retry.retry(Retry.Policy.immediate.maxRetries(2)) {
            attempts += 1
            Future.failed(if (attempts == 3) finalError else AttemptError(attempts, 500))
          }
        )
        .exit
        .map { exit =>
          val sameError = exit match {
            case Exit.Failure(cause) => cause.failureOption.exists(_ eq finalError)
            case Exit.Success(_)     => false
          }
          assertTrue(attempts == 3, sameError)
        }
    },
    test("never gives up on the first failure") {
      val error    = AttemptError(1, 500)
      var attempts = 0
      ZIO
        .fromFuture(_ =>
          Retry.retry(Retry.Policy.never) {
            attempts += 1
            Future.failed(error)
          }
        )
        .exit
        .map(exit => assertTrue(attempts == 1, exit == Exit.fail(error)))
    },
    test("periodic, fibonacci, sequencing, union, and intersection keep independent state") {
      val errors = List.tabulate(8)(index => AttemptError(index + 1, 500))
      for {
        sequenced <- run(
                       Retry.Policy
                         .periodic(3.seconds)
                         .maxRetries(1)
                         .andThen(
                           Retry.Policy.fibonacci(8.seconds, 13.seconds).maxRetries(2)
                         ),
                       errors.take(3)
                     )
        unioned <- run(
                     Retry.Policy.immediate.maxRetries(1).union(Retry.Policy.periodic(10.seconds).maxRetries(3)),
                     errors.take(3)
                   )
        intersected <- run(
                         Retry.Policy
                           .periodic(2.seconds)
                           .maxRetries(1)
                           .intersect(
                             Retry.Policy.periodic(7.seconds).maxRetries(2)
                           ),
                         errors.take(1)
                       )
      } yield assertTrue(
        sequenced == (4   -> List(3.seconds, 8.seconds, 13.seconds)),
        unioned == (4     -> List(Duration.Zero, 10.seconds, 10.seconds)),
        intersected == (2 -> List(7.seconds))
      )
    },
    test("filtered branch sees each changing failure without consuming state on a non-match") {
      val policy = Retry.Policy
        .periodic(9.seconds)
        .onlyWhen(Retry.Props.statusCode.eq(500))
        .union(Retry.Policy.periodic(2.seconds).maxRetries(3))
      run(
        policy,
        List(AttemptError(1, 500), AttemptError(2, 400), AttemptError(3, 500))
      ).map { case (attempts, delays) =>
        assertTrue(attempts == 4, delays == List(2.seconds, 2.seconds, 2.seconds))
      }
    },
    test("time box gives up at the inclusive elapsed-time boundary") {
      val runtime  = new TestRuntime()
      var attempts = 0
      ZIO
        .fromFuture(_ =>
          LocalRetry.retryWith(
            Retry.Policy.periodic(5.seconds).within(10.seconds),
            _ => Nil,
            () => {
              attempts += 1
              Future.failed(AttemptError(attempts, 500))
            },
            runtime
          )
        )
        .exit
        .map { exit =>
          assertTrue(attempts == 3, runtime.sleeps.toList == List(5.seconds, 5.seconds), exit.isFailure)
        }
    },
    test("clamp, delay addition, and positive jitter compose deterministically") {
      val runtime = new TestRuntime(randomValue = 0.5)
      run(
        Retry.Policy.periodic(20.seconds).clamp(3.seconds, 7.seconds).addDelay(2.seconds).withJitter(0.4).maxRetries(1),
        List(AttemptError(1, 500)),
        runtime
      ).map { case (_, delays) =>
        assertTrue(delays == List(10800.millis))
      }
    },
    test("nonfinite exponential multipliers produce zero delay before clamping") {
      run(
        Retry.Policy.exponential(1.millis, 1e308).clamp(Duration.Zero, 1.second).maxRetries(3),
        List.tabulate(3)(index => AttemptError(index + 1, 500))
      ).map { case (_, delays) =>
        assertTrue(delays == List(1.millis, 1.second, Duration.Zero))
      }
    },
    test("long timer delays are capped into safe chunks") {
      assertTrue(
        LocalRetry.nextTimerDelay(Int.MaxValue.millis) == Int.MaxValue.millis,
        LocalRetry.nextTimerDelay(30.days) == Int.MaxValue.millis
      )
    },
    test("all predicate forms use properties projected from the current failure") {
      val projected: Throwable => Iterable[Retry.Property] = {
        case _: AttemptError =>
          List(
            Retry.Props.statusCode -> "503",
            Retry.Props.errorType  -> "transient-timeout",
            Retry.Props.trapType   -> true
          )
        case _ => Nil
      }
      val predicate = Retry.Props.statusCode
        .eq(503)
        .and(Retry.Props.statusCode.neq(502))
        .and(Retry.Props.statusCode.gt(500))
        .and(Retry.Props.statusCode.gte(503))
        .and(Retry.Props.statusCode.lt(504))
        .and(Retry.Props.statusCode.lte(503))
        .and(
          Retry.Predicate.OneOf(
            "status-code",
            List(Retry.PredicateValue.boolean(true), Retry.PredicateValue.integer(503))
          )
        )
        .and(Retry.Props.errorType.matchesGlob("transient-*"))
        .and(Retry.Props.errorType.startsWith("transient"))
        .and(Retry.Props.errorType.contains("timeout"))
        .and(Retry.Props.trapType.exists)
        .and(Retry.Predicate.never.not)
        .or(Retry.Predicate.never)

      run(
        Retry.Policy.immediate.onlyWhen(predicate).maxRetries(1),
        List(AttemptError(1, 0)),
        properties = projected
      ).map { case (attempts, _) => assertTrue(attempts == 2) }
    },
    test("local glob predicates use the documented no-flags regex approximation") {
      val cases = List(
        ("service-*", "service-api", true),
        ("service-?", "service-a", true),
        ("service-?", "service-api", false),
        ("service-[a-c]", "service-b", false),
        ("service-[a-c]", "service-[a-c]", true),
        ("*", "line1\nline2", false),
        ("?", "\n", false),
        ("?", "😀", false),
        ("??", "😀", true)
      )

      ZIO
        .foreach(cases) { case (pattern, value, expected) =>
          run(
            Retry.Policy
              .periodic(1.second)
              .onlyWhen(Retry.Props.errorType.matchesGlob(pattern))
              .union(Retry.Policy.periodic(9.seconds).maxRetries(1)),
            List(AttemptError(1, 0)),
            properties = _ => List(Retry.Props.errorType -> value)
          ).map { case (_, delays) => (delays == List(1.second)) == expected }
        }
        .map(results => assertTrue(results.forall(identity)))
    },
    test("predicate evaluation errors propagate through composition without advancing branches") {
      val missing    = Retry.Policy.immediate.onlyWhen(Retry.Props.statusCode.eq(500))
      val oneOfError = Retry.Policy.immediate.onlyWhen(
        Retry.Predicate
          .OneOf(
            "status-code",
            List(Retry.PredicateValue.boolean(true), Retry.PredicateValue.integer(503))
          )
          .not
      )
      val emptyOneOf = Retry.Policy.immediate.onlyWhen(Retry.Predicate.OneOf("status-code", Nil).not).maxRetries(1)

      def attempts(policy: Retry.Policy, properties: Iterable[Retry.Property]): zio.UIO[Int] = {
        var count = 0
        ZIO
          .fromFuture(_ =>
            Retry.retry(policy, _ => properties) {
              count += 1
              Future.failed(AttemptError(count, 500))
            }
          )
          .exit
          .as(count)
      }

      for {
        union       <- attempts(missing.union(Retry.Policy.immediate.maxRetries(1)), Nil)
        andThen     <- attempts(missing.andThen(Retry.Policy.immediate.maxRetries(1)), Nil)
        memberError <-
          attempts(oneOfError.union(Retry.Policy.immediate.maxRetries(1)), List(Retry.Props.statusCode -> 500))
        emptyMember <- attempts(emptyOneOf, List(Retry.Props.statusCode -> 500))
        outOfRange  <- attempts(
                        Retry.Policy.immediate.onlyWhen(Retry.Props.statusCode.gt(0)).union(Retry.Policy.immediate),
                        List(Retry.Props.statusCode -> "9223372036854775808")
                      )
        unicodeDigit <- attempts(
                          Retry.Policy.immediate.onlyWhen(Retry.Props.statusCode.gt(0)).union(Retry.Policy.immediate),
                          List(Retry.Props.statusCode -> "١")
                        )
      } yield assertTrue(
        union == 1,
        andThen == 1,
        memberError == 1,
        emptyMember == 2,
        outOfRange == 1,
        unicodeDigit == 1
      )
    },
    test("raw policies execute and malformed or cyclic raw ASTs fail before the operation") {
      val valid = JsRetryPolicyTree(
        scala.scalajs.js.Array(
          JsPolicyNode.countBox(
            golem.host.js.JsCountBoxConfig(1, 1)
          ),
          JsPolicyNode.immediate
        )
      )
      val invalid = JsRetryPolicyTree(
        scala.scalajs.js.Array(
          JsPolicyNode.countBox(
            golem.host.js.JsCountBoxConfig(1, 99)
          )
        )
      )
      val cyclic = JsRetryPolicyTree(
        scala.scalajs.js.Array(
          JsPolicyNode.countBox(
            golem.host.js.JsCountBoxConfig(1, 0)
          )
        )
      )
      val nullNodes  = JsRetryPolicyTree(null)
      var attempts   = 0
      val nullResult =
        try Some(Retry.retry(nullNodes)(Future.successful("unused")))
        catch { case _: Throwable => None }

      for {
        value <- ZIO.fromFuture(_ =>
                   Retry.retry(valid) {
                     attempts += 1
                     if (attempts == 1) Future.failed(AttemptError(1, 500)) else Future.successful("ok")
                   }
                 )
        invalidExit <- ZIO.fromFuture(_ => Retry.retry(invalid)(Future.successful("unused"))).exit
        cyclicExit  <- ZIO.fromFuture(_ => Retry.retry(cyclic)(Future.successful("unused"))).exit
        nullExit    <- ZIO.fromFuture(_ => nullResult.get).exit
      } yield assertTrue(
        value == "ok",
        attempts == 2,
        invalidExit.isFailure,
        cyclicExit.isFailure,
        nullResult.isDefined,
        nullExit.isFailure
      )
    },
    test("synchronous failures are retried and the exact final throwable is propagated") {
      val finalError = AttemptError(2, 500)
      var attempts   = 0
      ZIO
        .fromFuture(_ =>
          Retry.retry(Retry.Policy.immediate.maxRetries(1)) {
            attempts += 1
            if (attempts == 1) throw AttemptError(1, 500) else throw finalError
          }
        )
        .exit
        .map { exit =>
          val sameError = exit match {
            case Exit.Failure(cause) => cause.failureOption.exists(_ eq finalError)
            case Exit.Success(_)     => false
          }
          assertTrue(attempts == 2, sameError)
        }
    }
  )
}
