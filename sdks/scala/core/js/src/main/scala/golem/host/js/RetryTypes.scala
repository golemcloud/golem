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

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// golem:api/retry@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

// --- PredicateValue  –  tagged union ---

@js.native
sealed trait JsPredicateValue extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsPredicateValueText extends JsPredicateValue {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsPredicateValueInteger extends JsPredicateValue {
  @JSName("val") def value: js.BigInt = js.native
}

@js.native
sealed trait JsPredicateValueBoolean extends JsPredicateValue {
  @JSName("val") def value: Boolean = js.native
}

object JsPredicateValue {
  def text(value: String): JsPredicateValue =
    JsShape.tagged[JsPredicateValue]("text", value.asInstanceOf[js.Any])

  def integer(value: js.BigInt): JsPredicateValue =
    JsShape.tagged[JsPredicateValue]("integer", value)

  def boolean(value: Boolean): JsPredicateValue =
    JsShape.tagged[JsPredicateValue]("boolean", value.asInstanceOf[js.Any])
}

// --- Shared predicate records ---

@js.native
sealed trait JsPropertyComparison extends js.Object {
  def propertyName: String    = js.native
  def value: JsPredicateValue = js.native
}

object JsPropertyComparison {
  def apply(propertyName: String, value: JsPredicateValue): JsPropertyComparison =
    js.Dynamic.literal("propertyName" -> propertyName, "value" -> value).asInstanceOf[JsPropertyComparison]
}

@js.native
sealed trait JsPropertySetCheck extends js.Object {
  def propertyName: String               = js.native
  def values: js.Array[JsPredicateValue] = js.native
}

object JsPropertySetCheck {
  def apply(propertyName: String, values: js.Array[JsPredicateValue]): JsPropertySetCheck =
    js.Dynamic
      .literal("propertyName" -> propertyName, "values" -> values)
      .asInstanceOf[JsPropertySetCheck]
}

@js.native
sealed trait JsPropertyPattern extends js.Object {
  def propertyName: String = js.native
  def pattern: String      = js.native
}

object JsPropertyPattern {
  def apply(propertyName: String, pattern: String): JsPropertyPattern =
    js.Dynamic.literal("propertyName" -> propertyName, "pattern" -> pattern).asInstanceOf[JsPropertyPattern]
}

// --- PredicateNode  –  tagged union ---

@js.native
sealed trait JsPredicateNode extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsPredicateNodeComparison extends JsPredicateNode {
  @JSName("val") def value: JsPropertyComparison = js.native
}

@js.native
sealed trait JsPredicateNodeStringValue extends JsPredicateNode {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsPredicateNodeSetCheck extends JsPredicateNode {
  @JSName("val") def value: JsPropertySetCheck = js.native
}

@js.native
sealed trait JsPredicateNodePattern extends JsPredicateNode {
  @JSName("val") def value: JsPropertyPattern = js.native
}

@js.native
sealed trait JsPredicateNodePair extends JsPredicateNode {
  @JSName("val") def value: js.Tuple2[Int, Int] = js.native
}

@js.native
sealed trait JsPredicateNodeIndex extends JsPredicateNode {
  @JSName("val") def value: Int = js.native
}

object JsPredicateNode {
  def propEq(value: JsPropertyComparison): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-eq", value)

  def propNeq(value: JsPropertyComparison): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-neq", value)

  def propGt(value: JsPropertyComparison): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-gt", value)

  def propGte(value: JsPropertyComparison): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-gte", value)

  def propLt(value: JsPropertyComparison): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-lt", value)

  def propLte(value: JsPropertyComparison): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-lte", value)

  def propExists(value: String): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-exists", value.asInstanceOf[js.Any])

  def propIn(value: JsPropertySetCheck): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-in", value)

  def propMatches(value: JsPropertyPattern): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-matches", value)

  def propStartsWith(value: JsPropertyPattern): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-starts-with", value)

  def propContains(value: JsPropertyPattern): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("prop-contains", value)

  def predAnd(left: Int, right: Int): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("pred-and", js.Tuple2(left, right))

  def predOr(left: Int, right: Int): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("pred-or", js.Tuple2(left, right))

  def predNot(inner: Int): JsPredicateNode =
    JsShape.tagged[JsPredicateNode]("pred-not", inner.asInstanceOf[js.Any])

  def predTrue: JsPredicateNode =
    JsShape.tagOnly[JsPredicateNode]("pred-true")

  def predFalse: JsPredicateNode =
    JsShape.tagOnly[JsPredicateNode]("pred-false")
}

// --- RetryPredicate ---

@js.native
sealed trait JsRetryPredicate extends js.Object {
  def nodes: js.Array[JsPredicateNode] = js.native
}

object JsRetryPredicate {
  def apply(nodes: js.Array[JsPredicateNode]): JsRetryPredicate =
    js.Dynamic.literal("nodes" -> nodes).asInstanceOf[JsRetryPredicate]
}

// --- PolicyNode  –  tagged union ---

@js.native
sealed trait JsPolicyNode extends js.Object {
  def tag: String = js.native
}

// --- Shared policy records ---

@js.native
sealed trait JsExponentialConfig extends js.Object {
  def baseDelay: js.BigInt = js.native
  def factor: Double       = js.native
}

object JsExponentialConfig {
  def apply(baseDelay: js.BigInt, factor: Double): JsExponentialConfig =
    js.Dynamic.literal("baseDelay" -> baseDelay, "factor" -> factor).asInstanceOf[JsExponentialConfig]
}

@js.native
sealed trait JsFibonacciConfig extends js.Object {
  def first: js.BigInt  = js.native
  def second: js.BigInt = js.native
}

object JsFibonacciConfig {
  def apply(first: js.BigInt, second: js.BigInt): JsFibonacciConfig =
    js.Dynamic.literal("first" -> first, "second" -> second).asInstanceOf[JsFibonacciConfig]
}

@js.native
sealed trait JsCountBoxConfig extends js.Object {
  def maxRetries: Double = js.native
  def inner: Int         = js.native
}

object JsCountBoxConfig {
  def apply(maxRetries: Double, inner: Int): JsCountBoxConfig =
    js.Dynamic.literal("maxRetries" -> maxRetries, "inner" -> inner).asInstanceOf[JsCountBoxConfig]
}

@js.native
sealed trait JsTimeBoxConfig extends js.Object {
  def limit: js.BigInt = js.native
  def inner: Int       = js.native
}

object JsTimeBoxConfig {
  def apply(limit: js.BigInt, inner: Int): JsTimeBoxConfig =
    js.Dynamic.literal("limit" -> limit, "inner" -> inner).asInstanceOf[JsTimeBoxConfig]
}

@js.native
sealed trait JsClampConfig extends js.Object {
  def minDelay: js.BigInt = js.native
  def maxDelay: js.BigInt = js.native
  def inner: Int          = js.native
}

object JsClampConfig {
  def apply(minDelay: js.BigInt, maxDelay: js.BigInt, inner: Int): JsClampConfig =
    js.Dynamic
      .literal("minDelay" -> minDelay, "maxDelay" -> maxDelay, "inner" -> inner)
      .asInstanceOf[JsClampConfig]
}

@js.native
sealed trait JsAddDelayConfig extends js.Object {
  def delay: js.BigInt = js.native
  def inner: Int       = js.native
}

object JsAddDelayConfig {
  def apply(delay: js.BigInt, inner: Int): JsAddDelayConfig =
    js.Dynamic.literal("delay" -> delay, "inner" -> inner).asInstanceOf[JsAddDelayConfig]
}

@js.native
sealed trait JsJitterConfig extends js.Object {
  def factor: Double = js.native
  def inner: Int     = js.native
}

object JsJitterConfig {
  def apply(factor: Double, inner: Int): JsJitterConfig =
    js.Dynamic.literal("factor" -> factor, "inner" -> inner).asInstanceOf[JsJitterConfig]
}

@js.native
sealed trait JsFilteredConfig extends js.Object {
  def predicate: JsRetryPredicate = js.native
  def inner: Int                  = js.native
}

object JsFilteredConfig {
  def apply(predicate: JsRetryPredicate, inner: Int): JsFilteredConfig =
    js.Dynamic.literal("predicate" -> predicate, "inner" -> inner).asInstanceOf[JsFilteredConfig]
}

@js.native
sealed trait JsPolicyNodeDuration extends JsPolicyNode {
  @JSName("val") def value: js.BigInt = js.native
}

@js.native
sealed trait JsPolicyNodeExponential extends JsPolicyNode {
  @JSName("val") def value: JsExponentialConfig = js.native
}

@js.native
sealed trait JsPolicyNodeFibonacci extends JsPolicyNode {
  @JSName("val") def value: JsFibonacciConfig = js.native
}

@js.native
sealed trait JsPolicyNodeCountBox extends JsPolicyNode {
  @JSName("val") def value: JsCountBoxConfig = js.native
}

@js.native
sealed trait JsPolicyNodeTimeBox extends JsPolicyNode {
  @JSName("val") def value: JsTimeBoxConfig = js.native
}

@js.native
sealed trait JsPolicyNodeClamp extends JsPolicyNode {
  @JSName("val") def value: JsClampConfig = js.native
}

@js.native
sealed trait JsPolicyNodeAddDelay extends JsPolicyNode {
  @JSName("val") def value: JsAddDelayConfig = js.native
}

@js.native
sealed trait JsPolicyNodeJitter extends JsPolicyNode {
  @JSName("val") def value: JsJitterConfig = js.native
}

@js.native
sealed trait JsPolicyNodeFiltered extends JsPolicyNode {
  @JSName("val") def value: JsFilteredConfig = js.native
}

@js.native
sealed trait JsPolicyNodePair extends JsPolicyNode {
  @JSName("val") def value: js.Tuple2[Int, Int] = js.native
}

object JsPolicyNode {
  def periodic(value: js.BigInt): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("periodic", value)

  def exponential(value: JsExponentialConfig): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("exponential", value)

  def fibonacci(value: JsFibonacciConfig): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("fibonacci", value)

  def immediate: JsPolicyNode =
    JsShape.tagOnly[JsPolicyNode]("immediate")

  def never: JsPolicyNode =
    JsShape.tagOnly[JsPolicyNode]("never")

  def countBox(value: JsCountBoxConfig): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("count-box", value)

  def timeBox(value: JsTimeBoxConfig): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("time-box", value)

  def clampDelay(value: JsClampConfig): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("clamp-delay", value)

  def addDelay(value: JsAddDelayConfig): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("add-delay", value)

  def jitter(value: JsJitterConfig): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("jitter", value)

  def filteredOn(value: JsFilteredConfig): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("filtered-on", value)

  def andThen(left: Int, right: Int): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("and-then", js.Tuple2(left, right))

  def policyUnion(left: Int, right: Int): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("policy-union", js.Tuple2(left, right))

  def policyIntersect(left: Int, right: Int): JsPolicyNode =
    JsShape.tagged[JsPolicyNode]("policy-intersect", js.Tuple2(left, right))
}

// --- RetryPolicyTree ---

@js.native
sealed trait JsRetryPolicyTree extends js.Object {
  def nodes: js.Array[JsPolicyNode] = js.native
}

object JsRetryPolicyTree {
  def apply(nodes: js.Array[JsPolicyNode]): JsRetryPolicyTree =
    js.Dynamic.literal("nodes" -> nodes).asInstanceOf[JsRetryPolicyTree]
}

// --- NamedRetryPolicy ---

@js.native
sealed trait JsNamedRetryPolicy extends js.Object {
  def name: String                = js.native
  def priority: Double            = js.native
  def predicate: JsRetryPredicate = js.native
  def policy: JsRetryPolicyTree   = js.native
}

object JsNamedRetryPolicy {
  def apply(
    name: String,
    priority: Double,
    predicate: JsRetryPredicate,
    policy: JsRetryPolicyTree
  ): JsNamedRetryPolicy =
    js.Dynamic
      .literal(
        "name"      -> name,
        "priority"  -> priority,
        "predicate" -> predicate,
        "policy"    -> policy
      )
      .asInstanceOf[JsNamedRetryPolicy]
}
