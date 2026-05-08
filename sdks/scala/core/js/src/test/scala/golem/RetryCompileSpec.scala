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

import golem.host.{Retry, RetryApi}
import zio.test._

object RetryCompileSpec extends ZIOSpecDefault {
  private val namedPolicy =
    Retry
      .named(
        "http-default",
        Retry.Policy.exponential(scala.concurrent.duration.DurationInt(1).second, 2.0)
      )
      .priority(10)
      .appliesWhen(
        Retry.Props.statusCode.gte(500).and(Retry.Props.errorType.eq("transient"))
      )

  private val property = Retry.Props.statusCode -> 503

  // Kept unreachable so Scala.js does not try to link host imports in the test runtime,
  // while the compiler still verifies the high-level overloads exist.
  private def compileOnlySurface(): Unit = {
    val retrySetter: Retry.NamedPolicy => Unit                                  = policy => RetryApi.setRetryPolicy(policy)
    val retryResolver: (String, String, Retry.Property) => Option[Retry.Policy] =
      (verb, nounUri, prop) => RetryApi.resolvePolicy(verb, nounUri, prop)
    val retryGuard: Retry.NamedPolicy => Guards.RetryPolicyGuard =
      policy => Guards.useRetryPolicy(policy)

    val _ = retrySetter
    val _ = retryResolver
    val _ = retryGuard
  }

  def spec = suite("RetryCompileSpec")(
    test("builder chain compiles and exposes fluent fields") {
      assertTrue(
        namedPolicy.name == "http-default",
        namedPolicy.priority == 10,
        property.name == "status-code",
        property.value == Retry.PredicateValue.integer(503)
      )
    },
    test("high-level retry overloads typecheck") {
      assertTrue(true)
    }
  )
}
