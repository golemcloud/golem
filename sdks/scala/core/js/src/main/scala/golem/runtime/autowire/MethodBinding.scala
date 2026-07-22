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

package golem.runtime.autowire

import golem.Principal
import golem.FutureInterop
import golem.host.js.schema.{JsAgentError, JsSchemaValueTree}
import golem.runtime.{InputRecordCodec, MethodMetadata, OutputCodec}

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js

/**
 * A wired agent method: decodes the `golem:agent@2.0.0` method input (a
 * `schema-value-tree` whose root encodes the parameter-list record) via its
 * [[InputRecordCodec]], invokes the handler, and encodes the result via its
 * [[OutputCodec]].
 *
 * The result is the host `option<schema-value-tree>`, modelled here as a Scala
 * [[Option]]: a `unit` output encodes [[None]] (host `none`); a `single` output
 * encodes `Some(tree)`. The guest export bridges this to / from `js.undefined`.
 */
trait MethodBinding[Instance] {
  def metadata: MethodMetadata

  def invoke(instance: Instance, input: JsSchemaValueTree, principal: Principal): js.Promise[Option[JsSchemaValueTree]]
}

object MethodBinding {
  def sync[Instance, In, Out](
    methodMetadata: MethodMetadata,
    inputCodec: InputRecordCodec[In],
    outputCodec: OutputCodec[Out]
  )(handler: (Instance, In, Principal) => Out): MethodBinding[Instance] =
    async[Instance, In, Out](methodMetadata, inputCodec, outputCodec)((instance, input, principal) =>
      Future.successful(handler(instance, input, principal))
    )

  def async[Instance, In, Out](
    methodMetadata: MethodMetadata,
    inputCodec: InputRecordCodec[In],
    outputCodec: OutputCodec[Out]
  )(handler: (Instance, In, Principal) => Future[Out]): MethodBinding[Instance] =
    new MethodBinding[Instance] {
      override val metadata: MethodMetadata = methodMetadata

      override def invoke(
        instance: Instance,
        input: JsSchemaValueTree,
        principal: Principal
      ): js.Promise[Option[JsSchemaValueTree]] = {
        val future =
          SchemaPayload
            .decode[In](input)(inputCodec)
            .fold(
              err =>
                Future
                  .failed[Option[JsSchemaValueTree]](js.JavaScriptException(JsAgentError.invalidInput(err.toString))),
              value =>
                handler(instance, value, principal).map { out =>
                  outputCodec.into match {
                    case None       => None
                    case Some(into) => Some(SchemaPayload.encode(out)(into))
                  }
                }
            )
        FutureInterop.toPromise(future)
      }
    }
}
