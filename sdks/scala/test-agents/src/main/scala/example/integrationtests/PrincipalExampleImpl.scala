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

import golem.Principal
import golem.runtime.annotations.agentImplementation

import scala.concurrent.Future

@agentImplementation()
final class PrincipalAgentImpl(input: String, principal: Principal) extends PrincipalAgent {
  private val creatorInfo: String = principal match {
    case Principal.Oidc(sub, issuer, _, email, _, _, _, _, _, _) =>
      s"OIDC user: sub=$sub, issuer=$issuer, email=${email.getOrElse("N/A")}"
    case Principal.Agent(componentId, agentId) =>
      s"Agent: componentId=$componentId, agentId=$agentId"
    case Principal.GolemUser(accountId) =>
      s"Golem user: accountId=$accountId"
    case Principal.Anonymous =>
      "Anonymous"
  }

  override def whoCreated(): Future[String] =
    Future.successful(s"Agent '$input' was created by: $creatorInfo")

  override def currentCaller(caller: Principal): Future[String] = {
    val callerInfo = caller match {
      case Principal.Anonymous                            => "anonymous"
      case Principal.Oidc(sub, _, _, _, _, _, _, _, _, _) => s"OIDC:$sub"
      case Principal.Agent(_, agentId)                    => s"agent:$agentId"
      case Principal.GolemUser(accountId)                 => s"user:$accountId"
    }
    Future.successful(s"Current caller: $callerInfo")
  }
}
