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

import golem.{InvocationContext, Principal}
import golem.runtime.annotations.agentImplementation

import scala.concurrent.Future

@agentImplementation()
final class PrincipalAgentImpl(input: String, context: InvocationContext) extends PrincipalAgent {
  private def describe(principal: Principal): String = principal match {
    case Principal.User(ref, _, _)        => s"user: $ref"
    case Principal.Account(ref, _, _)     => s"account: $ref"
    case Principal.Agent(ref, _, _, _, _) => s"agent: $ref"
    case Principal.Tool(ref, _, toolName) => s"tool: $toolName ($ref)"
    case Principal.Anonymous              => "anonymous"
  }

  // The constructor's context belongs to the constructor call, so the creator is read now and kept.
  private val creator: String = describe(context.caller.original)

  override def whoCreated(): Future[String] =
    Future.successful(s"Agent '$input' was created by: $creator")

  override def currentCaller(caller: InvocationContext): Future[String] =
    Future.successful(s"Current caller: ${describe(caller.caller.immediate)}")
}
