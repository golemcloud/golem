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

import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.annotation.JSImport

/**
 * Scala.js facade for `golem:api/retry@1.5.0`.
 *
 * Provides typed access to the semantic retry policy API. The tree-based policy
 * and predicate types are kept opaque — the SDK just passes them through
 * to/from the host.
 */
object RetryApi {

  def getRetryPolicies(): List[JsNamedRetryPolicy] =
    RetryModule.getRetryPolicies().toList

  def getRetryPolicyByName(name: String): Option[JsNamedRetryPolicy] =
    RetryModule.getRetryPolicyByName(name).toOption

  def resolveRetryPolicy(
    verb: String,
    nounUri: String,
    properties: List[(String, JsPredicateValue)]
  ): Option[JsRetryPolicyTree] =
    RetryModule.resolveRetryPolicy(verb, nounUri, properties.map(t => js.Tuple2(t._1, t._2)).toJSArray).toOption

  def setRetryPolicy(policy: JsNamedRetryPolicy): Unit =
    RetryModule.setRetryPolicy(policy)

  def removeRetryPolicy(name: String): Unit =
    RetryModule.removeRetryPolicy(name)

  // ---------------------------------------------------------------------------
  // Native bindings
  // ---------------------------------------------------------------------------

  @js.native
  @JSImport("golem:api/retry@1.5.0", JSImport.Namespace)
  private object RetryModule extends js.Object {
    def getRetryPolicies(): js.Array[JsNamedRetryPolicy]                   = js.native
    def getRetryPolicyByName(name: String): js.UndefOr[JsNamedRetryPolicy] = js.native
    def resolveRetryPolicy(
      verb: String,
      nounUri: String,
      properties: js.Array[js.Tuple2[String, JsPredicateValue]]
    ): js.UndefOr[JsRetryPolicyTree]                     = js.native
    def setRetryPolicy(policy: JsNamedRetryPolicy): Unit = js.native
    def removeRetryPolicy(name: String): Unit            = js.native
  }

  def raw: Any = RetryModule
}
