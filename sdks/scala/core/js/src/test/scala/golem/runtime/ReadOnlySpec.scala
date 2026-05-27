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

package golem.runtime

import golem.{BaseAgent, Principal}
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
    def withPrincipal(p: Principal, name: String): Future[String]

    def notReadOnly(name: String): Future[String]
  }

  @agentImplementation()
  final class ReadOnlyTestAgentImpl() extends ReadOnlyTestAgent {
    override def defaultCache(name: String): Future[String]                = Future.successful(name)
    override def noCache(name: String): Future[String]                     = Future.successful(name)
    override def untilWrite(name: String): Future[String]                  = Future.successful(name)
    override def ttl(name: String): Future[String]                         = Future.successful(name)
    override def withPrincipal(p: Principal, name: String): Future[String] = Future.successful(name)
    override def notReadOnly(name: String): Future[String]                 = Future.successful(name)
  }

  private lazy val defn: AgentDefinition[ReadOnlyTestAgent] =
    AgentImplementation.registerClass[ReadOnlyTestAgent, ReadOnlyTestAgentImpl]

  private def readOnlyDynOf(name: String): js.Dynamic = {
    val method  = defn.agentType.methods.find(_.name == name).get
    val asAny   = method.asInstanceOf[js.Dynamic]
    val ro      = asAny.readOnly
    ro
  }

  def spec = suite("ReadOnlySpec")(
    test("@readOnly() with default cache policy → until-write, usesPrincipal=false") {
      val ro = readOnlyDynOf("defaultCache")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "until-write",
        ro.usesPrincipal.asInstanceOf[Boolean] == false
      )
    },
    test("@readOnly(cache = \"no-cache\") → no-cache") {
      val ro = readOnlyDynOf("noCache")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "no-cache",
        ro.usesPrincipal.asInstanceOf[Boolean] == false
      )
    },
    test("@readOnly(cache = \"until-write\") → until-write") {
      val ro = readOnlyDynOf("untilWrite")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "until-write",
        ro.usesPrincipal.asInstanceOf[Boolean] == false
      )
    },
    test("@readOnly(cache = \"ttl(30 seconds)\") → ttl with 30s in nanoseconds") {
      val ro = readOnlyDynOf("ttl")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "ttl",
        ro.cachePolicy.selectDynamic("val").toString == "30000000000"
      )
    },
    test("usesPrincipal=true when the method has a Principal parameter") {
      val ro = readOnlyDynOf("withPrincipal")
      assertTrue(
        ro.cachePolicy.tag.asInstanceOf[String] == "until-write",
        ro.usesPrincipal.asInstanceOf[Boolean] == true
      )
    },
    test("methods without @readOnly have no readOnly metadata") {
      val method = defn.agentType.methods.find(_.name == "notReadOnly").get
      val asAny  = method.asInstanceOf[js.Dynamic]
      assertTrue(js.isUndefined(asAny.readOnly))
    }
  )
}
