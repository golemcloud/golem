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

import golem.HostApi
import golem.host.RetryApi
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class GuardsDemoImpl(@unused private val name: String) extends GuardsDemo {

  private val expectedCliRetryPolicyNames = List(
    "scala-integration-immediate",
    "scala-integration-never"
  )

  private def appendRetryPolicyVisibility(sb: StringBuilder): Unit = {
    val policies = RetryApi.getRetryPolicies().sortBy(_.name)
    val missing  = expectedCliRetryPolicyNames.filterNot(name => RetryApi.getRetryPolicyByName(name).isDefined)

    sb.append(s"original retry policies count=${policies.size}\n")
    sb.append(s"visible retry policies=${policies.map(_.name).mkString(",")}\n")
    if (missing.isEmpty) sb.append("result=retry-visible-ok\n")
    else sb.append(s"result=retry-visible-missing (${missing.mkString(",")})\n")
  }

  override def guardsBlockDemo(): Future[String] = {
    val sb = new StringBuilder
    sb.append("=== Block-scoped Guards Demo ===\n")

    appendRetryPolicyVisibility(sb)

    Future.successful(sb.result())
  }

  override def guardsResourceDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Resource-style Guards Demo ===\n")

    appendRetryPolicyVisibility(sb)

    sb.result()
  }

  override def oplogDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Oplog Management Demo ===\n")

    HostApi.oplogCommit(1)
    sb.append("oplogCommit(1) completed\n")

    sb.result()
  }
}
