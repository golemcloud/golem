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

package golem.runtime.autowire

import golem.Principal
import golem.data.GolemSchema
import golem.host.js._
import golem.runtime.MethodMetadata
import golem.FutureInterop

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js

trait MethodBinding[Instance] {
  def metadata: MethodMetadata

  def inputSchema: JsDataSchema

  def outputSchema: JsDataSchema

  def invoke(instance: Instance, payload: JsDataValue, principal: Principal): js.Promise[JsDataValue]
}

object MethodBinding {
  def sync[Instance, In, Out](methodMetadata: MethodMetadata)(
    handler: (Instance, In, Principal) => Out
  )(implicit inSchema: GolemSchema[In], outSchema: GolemSchema[Out]): MethodBinding[Instance] =
    async[Instance, In, Out](methodMetadata)((instance, input, principal) =>
      Future.successful(handler(instance, input, principal))
    )

  def async[Instance, In, Out](methodMetadata: MethodMetadata)(
    handler: (Instance, In, Principal) => Future[Out]
  )(implicit inSchema: GolemSchema[In], outSchema: GolemSchema[Out]): MethodBinding[Instance] =
    new MethodBinding[Instance] {
      override val metadata: MethodMetadata   = methodMetadata
      override val inputSchema: JsDataSchema  = HostPayload.schema[In]
      override val outputSchema: JsDataSchema = HostPayload.schema[Out]

      override def invoke(instance: Instance, payload: JsDataValue, principal: Principal): js.Promise[JsDataValue] = {
        val future =
          HostPayload
            .decode[In](payload)
            .fold(
              err => Future.failed(js.JavaScriptException(JsAgentError.invalidInput(err))),
              value =>
                handler(instance, value, principal).flatMap { out =>
                  HostPayload.encode[Out](out) match {
                    case Left(error) => Future.failed(js.JavaScriptException(JsAgentError.invalidInput(error)))
                    case Right(data) => Future.successful(data)
                  }
                }
            )

        FutureInterop.toPromise(future)
      }
    }
}
