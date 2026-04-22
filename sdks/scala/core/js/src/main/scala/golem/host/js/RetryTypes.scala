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

// ---------------------------------------------------------------------------
// golem:api/retry@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

// --- PredicateValue  –  tagged union ---

@js.native
sealed trait JsPredicateValue extends js.Object {
  def tag: String = js.native
}

// --- PredicateNode  –  tagged union ---

@js.native
sealed trait JsPredicateNode extends js.Object {
  def tag: String = js.native
}

// --- RetryPredicate ---

@js.native
sealed trait JsRetryPredicate extends js.Object {
  def nodes: js.Array[JsPredicateNode] = js.native
}

// --- PolicyNode  –  tagged union ---

@js.native
sealed trait JsPolicyNode extends js.Object {
  def tag: String = js.native
}

// --- RetryPolicyTree ---

@js.native
sealed trait JsRetryPolicyTree extends js.Object {
  def nodes: js.Array[JsPolicyNode] = js.native
}

// --- NamedRetryPolicy ---

@js.native
sealed trait JsNamedRetryPolicy extends js.Object {
  def name: String                = js.native
  def priority: Int               = js.native
  def predicate: JsRetryPredicate = js.native
  def policy: JsRetryPolicyTree   = js.native
}
