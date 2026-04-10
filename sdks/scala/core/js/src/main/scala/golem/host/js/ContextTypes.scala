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
// golem:api/context@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

// --- AttributeValue  –  tagged union ---

@js.native
sealed trait JsAttributeValue extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsAttributeValueString extends JsAttributeValue {
  @JSName("val") def value: String = js.native
}

object JsAttributeValue {
  def string(value: String): JsAttributeValue =
    JsShape.tagged[JsAttributeValue]("string", value.asInstanceOf[js.Any])
}

// --- Attribute ---

@js.native
sealed trait JsAttribute extends js.Object {
  def key: String             = js.native
  def value: JsAttributeValue = js.native
}

object JsAttribute {
  def apply(key: String, value: JsAttributeValue): JsAttribute =
    js.Dynamic.literal("key" -> key, "value" -> value).asInstanceOf[JsAttribute]
}

// --- AttributeChain ---

@js.native
sealed trait JsAttributeChain extends js.Object {
  def key: String                        = js.native
  def values: js.Array[JsAttributeValue] = js.native
}

object JsAttributeChain {
  def apply(key: String, values: js.Array[JsAttributeValue]): JsAttributeChain =
    js.Dynamic.literal("key" -> key, "values" -> values).asInstanceOf[JsAttributeChain]
}

// --- Span resource ---

@js.native
sealed trait JsSpan extends js.Object {
  def startedAt(): JsDatetime                                   = js.native
  def setAttribute(name: String, value: JsAttributeValue): Unit = js.native
  def setAttributes(attributes: js.Array[JsAttribute]): Unit    = js.native
  def finish(): Unit                                            = js.native
}

// --- InvocationContext resource ---

@js.native
sealed trait JsInvocationContext extends js.Object {
  def traceId(): String                                                           = js.native
  def spanId(): String                                                            = js.native
  def parent(): js.UndefOr[JsInvocationContext]                                   = js.native
  def getAttribute(key: String, inherited: Boolean): js.UndefOr[JsAttributeValue] = js.native
  def getAttributes(inherited: Boolean): js.Array[JsAttribute]                    = js.native
  def getAttributeChain(key: String): js.Array[JsAttributeValue]                  = js.native
  def getAttributeChains(): js.Array[JsAttributeChain]                            = js.native
  def traceContextHeaders(): js.Array[js.Tuple2[String, String]]                  = js.native
}
