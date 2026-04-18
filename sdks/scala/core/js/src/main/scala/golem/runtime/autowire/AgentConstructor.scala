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
import golem.data._
import golem.host.js._
import golem.runtime.ConstructorMetadata
import golem.FutureInterop

import scala.concurrent.Future
import scala.scalajs.js

trait AgentConstructor[Instance] {
  def info: ConstructorMetadata

  def schema: JsDataSchema

  def initialize(payload: JsDataValue, principal: Principal): js.Promise[Instance]
}

object AgentConstructor {
  def asyncJs[A, Instance](ctorInfo: ConstructorMetadata)(build: (A, Principal) => js.Promise[Instance])(implicit
    codec: GolemSchema[A]
  ): AgentConstructor[Instance] =
    async[A, Instance](ctorInfo)((a, principal) => FutureInterop.fromPromise(build(a, principal)))

  def noArgs[Instance](description: String, prompt: Option[String] = None)(build: Principal => Instance)(implicit
    codec: GolemSchema[Unit]
  ): AgentConstructor[Instance] =
    sync[Unit, Instance](ConstructorMetadata(name = None, description = description, promptHint = prompt))(
      (_, principal) => build(principal)
    )

  def sync[A, Instance](ctorInfo: ConstructorMetadata)(build: (A, Principal) => Instance)(implicit
    codec: GolemSchema[A]
  ): AgentConstructor[Instance] =
    async[A, Instance](ctorInfo)((a, principal) => Future.successful(build(a, principal)))

  def async[A, Instance](
    ctorInfo: ConstructorMetadata
  )(build: (A, Principal) => Future[Instance])(implicit codec: GolemSchema[A]): AgentConstructor[Instance] =
    new AgentConstructor[Instance] {
      override val info: ConstructorMetadata = ctorInfo
      override val schema: JsDataSchema      = HostSchemaEncoder.encode(codec.schema)

      override def initialize(payload: JsDataValue, principal: Principal): js.Promise[Instance] =
        HostValueDecoder
          .decode(codec.schema, payload)
          .flatMap(codec.decode)
          .fold(
            err =>
              js.Promise
                .reject(JsAgentError.invalidInput(err).asInstanceOf[Any])
                .asInstanceOf[js.Promise[Instance]],
            value => FutureInterop.toPromise(build(value, principal))
          )
    }
}
