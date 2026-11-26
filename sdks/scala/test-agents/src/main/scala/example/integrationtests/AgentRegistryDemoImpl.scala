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

package example.integrationtests

import example.templates.CounterClient
import golem.HostApi
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

    // registeredAgentTypeFor
    val selfMeta = HostApi.getSelfMetadata()
    val selfType = HostApi.registeredAgentTypeFor(selfMeta.agentId)
    sb.append(s"registeredAgentTypeFor('${selfMeta.agentId}')=$selfType\n")

    // resolveComponentId
    try {
      val componentId = HostApi.resolveComponentId("self")
      sb.append(s"resolveComponentId('self')=$componentId\n")
    } catch {
      case e: Throwable => sb.append(s"resolveComponentId('self') error: ${e.getMessage}\n")
    }

    sb.result()
  }

  override def exploreAgentQuery(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Agent Query Demo ===\n")

    // getSelfMetadata
    val self = HostApi.getSelfMetadata()
    sb.append(s"self.agentType=${self.agentTypeName.getOrElse("")}\n")
    sb.append(s"self.agentId=${self.agentId}\n")
    sb.append(s"self.retryCount=${self.retryCount}\n")
    sb.append(s"self.componentRevision=${self.componentRevision}\n")
    sb.append(s"self.args=${self.args}\n")
    sb.append(s"self.env.size=${self.env.size}\n")
    sb.append(s"self.config.size=${self.config.size}\n")

    // getAgentMetadata for self
    val selfAgentMeta = HostApi.getAgentMetadata(self.agentId)
    sb.append(s"getAgentMetadata(self.agentId)=${selfAgentMeta.map(_.agentId)}\n")

    // getAgents + nextAgentBatch
    try {
      val handle = HostApi.getAgents(self.agentTypeName.getOrElse(""), None, false)
      sb.append(s"getAgents handle obtained\n")
      val batch = HostApi.nextAgentBatch(handle)
      sb.append(s"nextAgentBatch count=${batch.map(_.size).getOrElse(0)}\n")
      batch.foreach { agents =>
        agents.take(3).foreach { a =>
          sb.append(s"  agent: ${a.agentId} (type=${a.agentTypeName.getOrElse("")})\n")
        }
        if (agents.size > 3) sb.append(s"  ... and ${agents.size - 3} more\n")
      }
    } catch {
      case e: Throwable => sb.append(s"getAgents error: ${e.getMessage}\n")
    }

    // generateIdempotencyKey
    val key1 = HostApi.generateIdempotencyKey()
    val key2 = HostApi.generateIdempotencyKey()
    sb.append(s"idempotencyKey1=$key1\n")
    sb.append(s"idempotencyKey2=$key2\n")
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
      val forkedId = HostApi.forkAgent(self.agentId, BigInt(2))
      sb.append(s"forkAgent: succeeded, forked agent $forkedId\n")
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

  override def createDemo(): Future[String] = {
    val sb = new StringBuilder
    sb.append("=== Create and getById RPC Demo ===\n")

    val result = for {
      created <- CounterClient.create("created-counter-instance")
      id      <- created.agentId()
      _        = sb.append(s"created counter $id\n")
      first   <- created.increment()
      again    = CounterClient.getById(id)
      second  <- again.increment()
    } yield {
      sb.append(s"counter.increment() = $first\n")
      sb.append(s"getById counter.increment() = $second\n")
      sb.result()
    }
    result.recover { case e: Throwable =>
      sb.append(s"create error: ${e.getMessage}\n")
      sb.result()
    }
  }

}
