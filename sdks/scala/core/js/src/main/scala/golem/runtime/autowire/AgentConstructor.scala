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
import golem.FutureInterop
import golem.host.js.schema.{JsAgentError, JsSchemaValueTree}
import golem.runtime.{ConstructorMetadata, InputRecordCodec}

import scala.concurrent.Future
import scala.scalajs.js

/**
 * A wired agent constructor: decodes the `golem:agent@2.0.0` constructor input
 * (a `schema-value-tree` whose root encodes the parameter-list record) via its
 * [[InputRecordCodec]] and builds an instance.
 */
trait AgentConstructor[Instance] {
  def info: ConstructorMetadata

  def initialize(input: JsSchemaValueTree, principal: Principal): js.Promise[Instance]
}

object AgentConstructor {

  def sync[A, Instance](info: ConstructorMetadata, inputCodec: InputRecordCodec[A])(
    build: (A, Principal) => Instance
  ): AgentConstructor[Instance] =
    async[A, Instance](info, inputCodec)((a, principal) => Future.successful(build(a, principal)))

  def async[A, Instance](info: ConstructorMetadata, inputCodec: InputRecordCodec[A])(
    build: (A, Principal) => Future[Instance]
  ): AgentConstructor[Instance] = {
    val ctorInfo = info
    new AgentConstructor[Instance] {
      override val info: ConstructorMetadata = ctorInfo

      override def initialize(input: JsSchemaValueTree, principal: Principal): js.Promise[Instance] =
        SchemaPayload
          .decode[A](input)(inputCodec)
          .fold(
            err =>
              js.Promise
                .reject(JsAgentError.invalidInput(err.toString).asInstanceOf[Any])
                .asInstanceOf[js.Promise[Instance]],
            value => FutureInterop.toPromise(build(value, principal))
          )
    }
  }
}
