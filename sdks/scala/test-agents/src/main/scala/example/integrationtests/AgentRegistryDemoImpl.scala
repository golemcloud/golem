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

import example.templates.CounterClient
import golem.{HostApi, Uuid}
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js

@agentImplementation()
final class AgentRegistryDemoImpl(@unused private val name: String) extends AgentRegistryDemo {

  override def exploreRegistry(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Agent Registry Demo ===\n")

    // registeredAgentType
    val counterType = HostApi.registeredAgentType("counter")
    sb.append(s"registeredAgentType('counter')=$counterType\n")
    counterType.foreach { rt =>
      sb.append(s"  typeName=${rt.typeName}\n")
    }

    val unknownType = HostApi.registeredAgentType("nonexistent-agent")
    sb.append(s"registeredAgentType('nonexistent-agent')=$unknownType\n")

    // getAllAgentTypes
    val allTypes = HostApi.getAllAgentTypes()
    sb.append(s"getAllAgentTypes count=${allTypes.size}\n")
    allTypes.foreach { t =>
      sb.append(s"  type: ${t.typeName}\n")
    }

    // parseAgentId
    val selfMeta = HostApi.getSelfMetadata()
    val parsed   = HostApi.parseAgentId(selfMeta.agentName)
    sb.append(s"parseAgentId('${selfMeta.agentName}')=$parsed\n")
    parsed.foreach { parts =>
      sb.append(s"  agentTypeName=${parts.agentTypeName}, phantom=${parts.phantom}\n")
    }

    val badParse = HostApi.parseAgentId("invalid-id-format")
    sb.append(s"parseAgentId('invalid-id-format')=$badParse\n")

    // resolveComponentId
    try {
      val componentId = HostApi.resolveComponentId("self")
      sb.append(s"resolveComponentId('self')=$componentId\n")
    } catch {
      case e: Throwable => sb.append(s"resolveComponentId('self') error: ${e.getMessage}\n")
    }

    // resolveAgentId / resolveAgentIdStrict
    try {
      val agentId = HostApi.resolveAgentId("self", "test-agent")
      sb.append(s"resolveAgentId('self','test-agent')=$agentId\n")
    } catch {
      case e: Throwable => sb.append(s"resolveAgentId error: ${e.getMessage}\n")
    }

    try {
      val strictId = HostApi.resolveAgentIdStrict("self", "test-agent-strict")
      sb.append(s"resolveAgentIdStrict('self','test-agent-strict')=$strictId\n")
    } catch {
      case e: Throwable => sb.append(s"resolveAgentIdStrict error: ${e.getMessage}\n")
    }

    sb.result()
  }

  override def exploreAgentQuery(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Agent Query Demo ===\n")

    // getSelfMetadata
    val self = HostApi.getSelfMetadata()
    sb.append(s"self.agentType=${self.agentType}\n")
    sb.append(s"self.agentName=${self.agentName}\n")
    sb.append(s"self.retryCount=${self.retryCount}\n")
    sb.append(s"self.componentRevision=${self.componentRevision}\n")
    sb.append(s"self.args=${self.args}\n")
    sb.append(s"self.env.size=${self.env.size}\n")
    sb.append(s"self.configVars.size=${self.configVars.size}\n")

    // getAgentMetadata for self
    val selfAgentMeta = HostApi.getAgentMetadata(self.agentId)
    sb.append(s"getAgentMetadata(self.agentId)=${selfAgentMeta.map(_.agentName)}\n")

    // getAgents + nextAgentBatch
    try {
      val handle = HostApi.getAgents(self.componentId, None, false)
      sb.append(s"getAgents handle obtained\n")
      val batch = HostApi.nextAgentBatch(handle)
      sb.append(s"nextAgentBatch count=${batch.map(_.size).getOrElse(0)}\n")
      batch.foreach { agents =>
        agents.take(3).foreach { a =>
          sb.append(s"  agent: ${a.agentName} (type=${a.agentType})\n")
        }
        if (agents.size > 3) sb.append(s"  ... and ${agents.size - 3} more\n")
      }
    } catch {
      case e: Throwable => sb.append(s"getAgents error: ${e.getMessage}\n")
    }

    // generateIdempotencyKey
    val key1 = HostApi.generateIdempotencyKey()
    val key2 = HostApi.generateIdempotencyKey()
    sb.append(s"idempotencyKey1=${key1.highBits},${key1.lowBits}\n")
    sb.append(s"idempotencyKey2=${key2.highBits},${key2.lowBits}\n")
    sb.append(s"keys are distinct=${key1 != key2}\n")

    sb.result()
  }

  override def exploreLifecycle(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Agent Lifecycle Demo ===\n")

    val self = HostApi.getSelfMetadata()

    // updateAgent (may not work locally)
    try {
      HostApi.updateAgent(self.agentId, self.componentRevision, HostApi.UpdateMode.Automatic)
      sb.append("updateAgent: succeeded\n")
    } catch {
      case e: Throwable => sb.append(s"updateAgent: ${e.getMessage}\n")
    }

    // forkAgent (may not work locally)
    try {
      val key        = HostApi.generateIdempotencyKey()
      val targetUuid = HostApi.UuidLiteral(
        js.BigInt(key.highBits.toString),
        js.BigInt(key.lowBits.toString)
      )
      val targetCid  = HostApi.ComponentIdLiteral(targetUuid)
      val targetAid  = HostApi.AgentIdLiteral(targetCid, "fork-target")
      val oplogIndex = HostApi.getOplogIndex()
      HostApi.forkAgent(self.agentId, targetAid, oplogIndex)
      sb.append("forkAgent: succeeded\n")
    } catch {
      case e: Throwable => sb.append(s"forkAgent: ${e.getMessage}\n")
    }

    // revertAgent (may not work locally)
    try {
      HostApi.revertAgent(self.agentId, HostApi.RevertAgentTarget.RevertLastInvocations(BigInt(0)))
      sb.append("revertAgent: succeeded\n")
    } catch {
      case e: Throwable => sb.append(s"revertAgent: ${e.getMessage}\n")
    }

    sb.result()
  }

  override def phantomDemo(): Future[String] = {
    val sb = new StringBuilder
    sb.append("=== Phantom RPC Demo ===\n")

    val phantomId = Uuid(BigInt(42), BigInt(99))
    sb.append(s"creating phantom counter with Uuid(42,99)\n")

    try {
      val counter = CounterClient.getPhantom("phantom-counter-instance", phantomId)
      sb.append(s"counter proxy created via getPhantom\n")

      counter.increment().map { result =>
        sb.append(s"counter.increment() = $result\n")
        sb.result()
      }
    } catch {
      case e: Throwable =>
        sb.append(s"phantom creation error: ${e.getMessage}\n")
        Future.successful(sb.result())
    }
  }
}
