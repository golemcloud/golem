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

package golem.runtime

import golem.{BaseAgent, InvocationContext}
import golem.runtime.annotations.{agentDefinition, agentImplementation, readOnly}
import golem.runtime.autowire.{AgentDefinition, AgentImplementation}
import zio._
import zio.test._

import scala.concurrent.Future
import scala.scalajs.js

object ReadOnlySpec extends ZIOSpecDefault {

  @agentDefinition("read-only-agent")
  trait ReadOnlyTestAgent extends BaseAgent {
    class Id()
    @readOnly()
    def defaultCache(name: String): Future[String]

    @readOnly(cache = "no-cache")
    def noCache(name: String): Future[String]

    @readOnly(cache = "until-write")
    def untilWrite(name: String): Future[String]

    @readOnly(cache = "ttl(30 seconds)")
    def ttl(name: String): Future[String]

    @readOnly()
    def withContext(context: InvocationContext, name: String): Future[String]

    def notReadOnly(name: String): Future[String]
  }

  @agentImplementation()
  final class ReadOnlyTestAgentImpl() extends ReadOnlyTestAgent {
    override def defaultCache(name: String): Future[String]                            = Future.successful(name)
    override def noCache(name: String): Future[String]                                 = Future.successful(name)
    override def untilWrite(name: String): Future[String]                              = Future.successful(name)
    override def ttl(name: String): Future[String]                                     = Future.successful(name)
    override def withContext(context: InvocationContext, name: String): Future[String] = Future.successful(name)
    override def notReadOnly(name: String): Future[String]                             = Future.successful(name)
  }

  private lazy val defn: AgentDefinition[ReadOnlyTestAgent] =
    AgentImplementation.registerClass[ReadOnlyTestAgent, ReadOnlyTestAgentImpl]

  private def readOnlyDynOf(name: String): js.Dynamic = {
    val method = defn.agentType.methods.find(_.name == name).get
    val asAny  = method.asInstanceOf[js.Dynamic]
    val ro     = asAny.readOnly
    ro
  }

  def spec = suite("ReadOnlySpec")(
    test("@readOnly() with default cache policy → until-write") {
      val ro = readOnlyDynOf("defaultCache")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "until-write"
      )
    },
    test("@readOnly(cache = \"no-cache\") → no-cache") {
      val ro = readOnlyDynOf("noCache")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "no-cache"
      )
    },
    test("@readOnly(cache = \"until-write\") → until-write") {
      val ro = readOnlyDynOf("untilWrite")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "until-write"
      )
    },
    test("@readOnly(cache = \"ttl(30 seconds)\") → ttl with 30s in nanoseconds") {
      val ro = readOnlyDynOf("ttl")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "ttl",
        ro.cachePolicy.selectDynamic("val").toString == "30000000000"
      )
    },
    test("a read-only method with an InvocationContext parameter keeps only its cache policy") {
      val ro = readOnlyDynOf("withContext")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "until-write",
        js.isUndefined(ro.selectDynamic("usesPrincipal"))
      )
    },
    test("methods without @readOnly have no readOnly metadata") {
      val method = defn.agentType.methods.find(_.name == "notReadOnly").get
      val asAny  = method.asInstanceOf[js.Dynamic]
      assertTrue(js.isUndefined(asAny.readOnly))
    }
  )
}
