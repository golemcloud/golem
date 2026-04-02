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

import golem.HostApi
import golem.host.{ContextApi, DurabilityApi}
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class ObservabilityDemoImpl(@unused private val name: String) extends ObservabilityDemo {

  override def traceDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Trace Demo ===\n")

    val parentSpan                      = ContextApi.startSpan("parent-operation")
    val parentTime: ContextApi.DateTime = parentSpan.startedAt()
    sb.append(s"Parent span started at ${parentTime.seconds}s ${parentTime.nanoseconds}ns\n")

    parentSpan.setAttribute("env", ContextApi.AttributeValue.StringValue("demo"))

    parentSpan.setAttributes(
      List(
        ContextApi.Attribute("service", ContextApi.AttributeValue.StringValue("observability-demo")),
        ContextApi.Attribute("version", ContextApi.AttributeValue.StringValue("1.0"))
      )
    )

    val childSpan = ContextApi.startSpan("child-operation")
    childSpan.setAttribute("step", ContextApi.AttributeValue.StringValue("processing"))
    childSpan.finish()
    sb.append("Child span created and finished.\n")

    val ctx: ContextApi.InvocationContext = ContextApi.currentContext()
    val traceId: String                   = ctx.traceId()
    val spanId: String                    = ctx.spanId()
    sb.append(s"Current context: traceId=$traceId spanId=$spanId\n")

    val parentCtx: Option[ContextApi.InvocationContext] = ctx.parent()
    sb.append(s"Parent context present: ${parentCtx.isDefined}\n")

    val attr: Option[ContextApi.AttributeValue] = ctx.getAttribute("env", true)
    sb.append(s"getAttribute('env', inherited=true): $attr\n")

    val attrs: List[ContextApi.Attribute] = ctx.getAttributes(false)
    sb.append(s"getAttributes(inherited=false): ${attrs.size} attributes\n")
    attrs.foreach { a =>
      val desc = a.value match {
        case ContextApi.AttributeValue.StringValue(v) => v
      }
      sb.append(s"  ${a.key} = $desc\n")
    }

    val chain: List[ContextApi.AttributeValue] = ctx.getAttributeChain("env")
    sb.append(s"getAttributeChain('env'): ${chain.size} values\n")

    val chains: List[ContextApi.AttributeChain] = ctx.getAttributeChains()
    sb.append(s"getAttributeChains(): ${chains.size} chains\n")
    chains.foreach { c =>
      sb.append(s"  chain '${c.key}': ${c.values.size} values\n")
    }

    val headers: List[(String, String)] = ctx.traceContextHeaders()
    sb.append(s"traceContextHeaders: ${headers.size} headers\n")
    headers.foreach { case (k, v) => sb.append(s"  $k: $v\n") }

    val prev = ContextApi.allowForwardingTraceContextHeaders(true)
    sb.append(s"allowForwardingTraceContextHeaders(true) => previous=$prev\n")
    ContextApi.allowForwardingTraceContextHeaders(prev)

    parentSpan.finish()
    sb.append("Parent span finished.\n")

    sb.toString()
  }

  override def durabilityDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Durability Demo ===\n")

    val state: DurabilityApi.DurableExecutionState = DurabilityApi.currentDurableExecutionState()
    sb.append(s"isLive: ${state.isLive}\n")
    val plTag = state.persistenceLevel match {
      case HostApi.PersistenceLevel.PersistNothing           => "persist-nothing"
      case HostApi.PersistenceLevel.PersistRemoteSideEffects => "persist-remote-side-effects"
      case HostApi.PersistenceLevel.Smart                    => "smart"
      case HostApi.PersistenceLevel.Unknown(tag)             => s"unknown($tag)"
    }
    sb.append(s"persistenceLevel: $plTag\n")

    val allTypes: List[DurabilityApi.DurableFunctionType] = List(
      DurabilityApi.DurableFunctionType.ReadLocal,
      DurabilityApi.DurableFunctionType.WriteLocal,
      DurabilityApi.DurableFunctionType.ReadRemote,
      DurabilityApi.DurableFunctionType.WriteRemote,
      DurabilityApi.DurableFunctionType.WriteRemoteBatched(None),
      DurabilityApi.DurableFunctionType.WriteRemoteBatched(Some(BigInt(42))),
      DurabilityApi.DurableFunctionType.WriteRemoteTransaction(None),
      DurabilityApi.DurableFunctionType.WriteRemoteTransaction(Some(BigInt(99)))
    )
    sb.append(s"DurableFunctionType variants: ${allTypes.size}\n")
    allTypes.foreach(ft => sb.append(s"  ${ft.tag}\n"))

    DurabilityApi.observeFunctionCall("example-iface", "test-function")
    sb.append("observeFunctionCall('example-iface', 'test-function') done\n")

    val beginIdx: DurabilityApi.OplogIndex =
      DurabilityApi.beginDurableFunction(DurabilityApi.DurableFunctionType.ReadLocal)
    sb.append(s"beginDurableFunction(ReadLocal) => oplogIndex=$beginIdx\n")

    DurabilityApi.endDurableFunction(DurabilityApi.DurableFunctionType.ReadLocal, beginIdx, false)
    sb.append(s"endDurableFunction(ReadLocal, $beginIdx, forced=false) done\n")

    sb.toString()
  }
}
