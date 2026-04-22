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

package golem.runtime.rpc

import golem.runtime.AgentType
import zio._
import zio.test._

import golem.host.js._

import scala.concurrent.Future
import scala.scalajs.js

object AgentClientBindListDoubleSpec extends ZIOSpecDefault {

  def spec = suite("AgentClientBindListDoubleSpec")(
    test("AgentClient.bind supports Unit method with List[Double] param (no missing JS function)") {
      ZIO.fromFuture { implicit ec =>
        val agentType =
          AgentClient.agentType[BindListDoubleWorkflow].asInstanceOf[AgentType[BindListDoubleWorkflow, Unit]]

        final class RecordingInvoker extends RpcInvoker {
          var triggered: Boolean = false

          override def invokeAndAwait(functionName: String, input: JsDataValue): Either[String, JsDataValue] =
            Left("not used")

          override def invoke(functionName: String, input: JsDataValue): Either[String, Unit] = {
            triggered = true
            Right(())
          }

          override def scheduleInvocation(
            datetime: golem.Datetime,
            functionName: String,
            input: JsDataValue
          ): Either[String, Unit] =
            Left("not used")

          override def scheduleCancelableInvocation(
            datetime: golem.Datetime,
            functionName: String,
            input: JsDataValue
          ): Either[String, CancellationToken] =
            Left("not used")
        }

        val invoker = new RecordingInvoker
        val remote  = RemoteAgentClient(agentType.typeName, "agent-1", null, invoker)

        val resolved =
          AgentClientRuntime.ResolvedAgent(
            agentType.asInstanceOf[AgentType[BindListDoubleWorkflow, Any]],
            remote
          )

        val client = AgentClient.bind[BindListDoubleWorkflow](resolved)

        client.finished(List(1.0, 2.0))

        Future.successful(invoker.triggered)
      }.map(triggered => assertTrue(triggered))
    }
  )
}
