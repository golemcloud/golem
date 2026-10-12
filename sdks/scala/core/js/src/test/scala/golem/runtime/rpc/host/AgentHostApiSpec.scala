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

package golem.runtime.rpc.host

import zio.test._

import scala.scalajs.js
import scala.util.Try

object AgentHostApiSpec extends ZIOSpecDefault {
  private def withHost[A](host: js.Object)(call: => A): A = {
    val global   = js.Dynamic.global.selectDynamic("globalThis")
    val previous = global.selectDynamic("__golemScalaTestHost")
    global.updateDynamic("__golemScalaTestHost")(host)
    try call
    finally global.updateDynamic("__golemScalaTestHost")(previous)
  }

  private def selfFailure(error: js.Any): Throwable =
    withHost(js.Dynamic.literal("getSelfMetadata" -> (() => throw js.JavaScriptException(error)))) {
      Try(AgentHostApi.getSelfMetadata()).failed.get
    }

  def spec = suite("AgentHostApiSpec")(
    test("self metadata permission denial explains the required permission without dumping fields") {
      val error   = js.Dynamic.literal("tag" -> "permission-denied", "secret" -> "do-not-print")
      val failure = selfFailure(error)
      assertTrue(
        failure.getMessage.contains("get-self-metadata"),
        failure.getMessage.contains("view permission on the running agent"),
        failure.getMessage.contains("effective permission card"),
        failure.getMessage.contains("initialCard upper bound"),
        !failure.getMessage.contains("[object Object]"),
        !failure.getMessage.contains("do-not-print"),
        failure.getCause.asInstanceOf[js.JavaScriptException].exception == error
      )
    },
    test("target metadata permission denial names the target operation") {
      withHost(
        js.Dynamic.literal(
          "getAgentMetadata" -> ((_: js.Any) =>
            throw js.JavaScriptException(js.Dynamic.literal("tag" -> "permission-denied"))
          )
        )
      ) {
        val failure = Try(AgentHostApi.getAgentMetadata(null)).failed.get
        assertTrue(
          failure.getMessage.contains("get-agent-metadata"),
          failure.getMessage.contains("view permission on the target agent")
        )
      }
    },
    test("self metadata backend failure retains its message and cause") {
      val error   = js.Dynamic.literal("tag" -> "backend-error", "val" -> "metadata unavailable")
      val failure = selfFailure(error)
      assertTrue(
        failure.getMessage == "get-self-metadata: backend error: metadata unavailable",
        failure.getCause.asInstanceOf[js.JavaScriptException].exception == error
      )
    },
    test("backend error does not stringify a non-string payload") {
      val failure =
        selfFailure(js.Dynamic.literal("tag" -> "backend-error", "val" -> js.Dynamic.literal("secret" -> "hidden")))
      assertTrue(failure.getMessage == "get-self-metadata: backend error")
    },
    test("native errors and unknown tagged errors remain unchanged") {
      val native         = new js.Error("native failure")
      val unknown        = js.Dynamic.literal("tag" -> "unknown", "secret" -> "hidden")
      val nativeFailure  = selfFailure(native)
      val unknownFailure = selfFailure(unknown)
      assertTrue(
        nativeFailure.asInstanceOf[js.JavaScriptException].exception == native,
        nativeFailure.getMessage.contains("native failure"),
        unknownFailure.asInstanceOf[js.JavaScriptException].exception == unknown
      )
    },
    test("successful self metadata and absent target metadata keep their return values") {
      val metadata = js.Dynamic.literal("agentId" -> js.Dynamic.literal("agentId" -> "self"))
      withHost(
        js.Dynamic.literal(
          "getSelfMetadata"  -> (() => metadata),
          "getAgentMetadata" -> ((_: js.Any) => js.undefined)
        )
      ) {
        val selfId = AgentHostApi.getSelfMetadata().agentId.agentId
        val target = AgentHostApi.getAgentMetadata(null)
        assertTrue(
          selfId == "self",
          target.isEmpty
        )
      }
    }
  ) @@ TestAspect.sequential
}
