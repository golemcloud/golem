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

package golem.host

import golem.host.js._
import zio.test._

import scala.concurrent.duration._

object RetryDslSpec extends ZIOSpecDefault {
  private val predicate =
    Retry.Props.statusCode
      .gte(500)
      .and(Retry.Props.errorType.oneOf("timeout", "transient"))
      .or(Retry.Props.trapType.eq("network").not)

  private val policy =
    Retry
      .named(
        "http-default",
        Retry.Policy
          .exponential(1.second, 2.0)
          .maxRetries(3)
          .within(30.seconds)
          .addDelay(250.millis)
          .withJitter(0.25)
          .onlyWhen(Retry.Props.errorType.eq("transient"))
          .andThen(Retry.Policy.periodic(5.seconds).maxRetries(1))
      )
      .priority(20)
      .appliesWhen(predicate)

  def spec = suite("RetryDslSpec")(
    test("named policy roundtrips through raw retry facades") {
      val roundtrip = Retry.NamedPolicy.toJs(policy).map(Retry.NamedPolicy.fromJs)
      assertTrue(roundtrip == Right(policy))
    },
    test("retry properties roundtrip through raw predicate values") {
      val propertyRoundtrip = Retry
        .propertiesToJs(
          List(
            Retry.Props.verb       -> "get",
            Retry.Props.statusCode -> 503,
            Retry.Props.trapType   -> Retry.PredicateValue.boolean(true)
          )
        )
        .map(
          _.toList.map(tuple => Retry.Property(tuple._1, Retry.PredicateValue.fromJs(tuple._2)))
        )

      assertTrue(
        propertyRoundtrip == Right(
          List(
            Retry.Property("verb", Retry.PredicateValue.text("get")),
            Retry.Property("status-code", Retry.PredicateValue.integer(503)),
            Retry.Property("trap-type", Retry.PredicateValue.boolean(true))
          )
        )
      )
    },
    test("rejects invalid exponential factors at conversion time") {
      val result = Retry.Policy.toJs(Retry.Policy.exponential(1.second, Double.PositiveInfinity))
      assertTrue(result.left.toOption.exists(_.message.contains("policy.factor must be finite")))
    },
    test("rejects invalid jitter factors at conversion time") {
      val result = Retry.Policy.toJs(Retry.Policy.immediate.withJitter(-0.1))
      assertTrue(result.left.toOption.exists(_.message.contains("policy.factor must be >= 0")))
    },
    test("rejects inverted clamp ranges at conversion time") {
      val result = Retry.Policy.toJs(Retry.Policy.immediate.clamp(5.seconds, 1.second))
      assertTrue(result.left.toOption.exists(_.message.contains("minDelay <= maxDelay")))
    },
    test("rejects integer predicate values outside the WIT s64 range") {
      val result = Retry.Predicate.toJs(Retry.Props.statusCode.eq(BigInt(Long.MaxValue) + 1))
      assertTrue(result.left.toOption.exists(_.message.contains("signed 64-bit integer")))
    },
    test("rejects negative durations at conversion time") {
      val result = Retry.Policy.toJs(Retry.Policy.periodic((-1).millis))
      assertTrue(result.left.toOption.exists(_.message.contains("must be >= 0")))
    },
    test("rejects negative named policy priorities at conversion time") {
      val result = Retry.NamedPolicy.toJs(Retry.named("bad", Retry.Policy.immediate).priority(-1))
      assertTrue(result.left.toOption.exists(_.message.contains("namedPolicy.priority must be >= 0")))
    },
    test("rejects malformed raw predicate indices during decode") {
      val malformed = JsRetryPredicate(
        scala.scalajs.js.Array(
          JsPredicateNode.predAnd(1, 99),
          JsPredicateNode.predTrue
        )
      )

      val failed =
        try {
          Retry.Predicate.fromJs(malformed)
          false
        } catch {
          case error: IllegalArgumentException =>
            error.getMessage.contains("predicate.root.right references predicate node 99")
        }

      assertTrue(
        failed
      )
    },
    test("rejects malformed raw named policy u32 values during decode") {
      val malformed = JsNamedRetryPolicy(
        "too-large-priority",
        4294967296d,
        JsRetryPredicate(scala.scalajs.js.Array(JsPredicateNode.predTrue)),
        JsRetryPolicyTree(scala.scalajs.js.Array(JsPolicyNode.immediate))
      )

      val failed =
        try {
          Retry.NamedPolicy.fromJs(malformed)
          false
        } catch {
          case error: IllegalArgumentException =>
            error.getMessage.contains("namedPolicy.priority must be a whole unsigned 32-bit integer")
        }

      assertTrue(
        failed
      )
    }
  )
}
