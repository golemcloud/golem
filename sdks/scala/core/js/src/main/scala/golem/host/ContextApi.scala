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
import scala.scalajs.js.annotation.JSImport

/**
 * Scala.js facade for `golem:api/context@1.5.0`.
 *
 * WIT interface:
 * {{{
 *   resource span { started-at, set-attribute, set-attributes, finish }
 *   resource invocation-context { trace-id, span-id, parent, get-attribute, get-attributes, ... }
 *   variant attribute-value { string(string) }
 *   record attribute { key: string, value: attribute-value }
 *   start-span: func(name: string) -> span
 *   current-context: func() -> invocation-context
 *   allow-forwarding-trace-context-headers: func(allow: bool) -> bool
 * }}}
 */
object ContextApi {

  // --- WIT: attribute-value variant ---

  sealed trait AttributeValue extends Product with Serializable
  object AttributeValue {
    final case class StringValue(value: String) extends AttributeValue

    def fromJs(raw: JsAttributeValue): AttributeValue =
      StringValue(raw.asInstanceOf[JsAttributeValueString].value)

    def toJs(av: AttributeValue): JsAttributeValue = av match {
      case StringValue(v) => JsAttributeValue.string(v)
    }
  }

  // --- WIT: attribute record ---

  final case class Attribute(key: String, value: AttributeValue)

  // --- WIT: attribute-chain record ---

  final case class AttributeChain(key: String, values: List[AttributeValue])

  // --- WIT: datetime record (shared timestamp type) ---

  final case class DateTime(seconds: BigInt, nanoseconds: Long)

  // --- WIT: span resource ---

  final class Span private[golem] (private[golem] val underlying: JsSpan) {

    def startedAt(): DateTime = {
      val raw   = underlying.startedAt()
      val secs  = BigInt(raw.seconds.toString)
      val nanos = raw.nanoseconds.toLong
      DateTime(secs, nanos)
    }

    def setAttribute(name: String, value: AttributeValue): Unit =
      underlying.setAttribute(name, AttributeValue.toJs(value))

    def setAttributes(attributes: List[Attribute]): Unit = {
      val arr = js.Array[JsAttribute]()
      attributes.foreach { a =>
        arr.push(JsAttribute(a.key, AttributeValue.toJs(a.value)))
      }
      underlying.setAttributes(arr)
    }

    def finish(): Unit =
      underlying.finish()
  }

  // --- WIT: invocation-context resource ---

  final class InvocationContext private[golem] (private[golem] val underlying: JsInvocationContext) {

    def traceId(): String =
      underlying.traceId()

    def spanId(): String =
      underlying.spanId()

    def parent(): Option[InvocationContext] =
      underlying.parent().toOption.map(p => new InvocationContext(p))

    def getAttribute(key: String, inherited: Boolean): Option[AttributeValue] =
      underlying.getAttribute(key, inherited).toOption.map(AttributeValue.fromJs)

    def getAttributes(inherited: Boolean): List[Attribute] =
      underlying.getAttributes(inherited).toList.map { a =>
        Attribute(a.key, AttributeValue.fromJs(a.value))
      }

    def getAttributeChain(key: String): List[AttributeValue] =
      underlying.getAttributeChain(key).toList.map(AttributeValue.fromJs)

    def getAttributeChains(): List[AttributeChain] =
      underlying.getAttributeChains().toList.map { c =>
        val key    = c.key
        val values = c.values.toList.map(AttributeValue.fromJs)
        AttributeChain(key, values)
      }

    def traceContextHeaders(): List[(String, String)] =
      underlying.traceContextHeaders().toList.map(kv => (kv._1, kv._2))
  }

  // --- Native bindings ---

  @js.native
  @JSImport("golem:api/context@1.5.0", JSImport.Namespace)
  private object ContextModule extends js.Object {
    def startSpan(name: String): JsSpan                             = js.native
    def currentContext(): JsInvocationContext                       = js.native
    def allowForwardingTraceContextHeaders(allow: Boolean): Boolean = js.native
  }

  // --- Typed public API ---

  def startSpan(name: String): Span =
    new Span(ContextModule.startSpan(name))

  def currentContext(): InvocationContext =
    new InvocationContext(ContextModule.currentContext())

  def allowForwardingTraceContextHeaders(allow: Boolean): Boolean =
    ContextModule.allowForwardingTraceContextHeaders(allow)

  def raw: Any = ContextModule
}
