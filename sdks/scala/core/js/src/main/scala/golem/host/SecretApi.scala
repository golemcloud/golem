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

package golem.host

import golem.host.js.JsSecretResource
import golem.host.js.schema.{JsSchemaGraph, JsSchemaValueTree}
import golem.runtime.autowire.SchemaPayload
import golem.schema.{FromSchema, GuestSecretHandle, IntoSchema}

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

object SecretApi {

  /** Reveal an owned secret by borrowing its handle. */
  def reveal[A](handle: GuestSecretHandle)(implicit into: IntoSchema[A], from: FromSchema[A]): A =
    SchemaPayload.decode[A](reveal(handle, SchemaPayload.graph[A])) match {
      case Right(value) => value
      case Left(error)  => throw new IllegalArgumentException(s"Failed to decode revealed secret: $error")
    }

  private[golem] def reveal(handle: GuestSecretHandle, expected: JsSchemaGraph): JsSchemaValueTree =
    handle
      .withHandle(raw => RevealModule.reveal(raw.asInstanceOf[JsSecretResource], expected))
      .getOrElse(throw new IllegalStateException("secret handle was already transferred"))

  @js.native
  @JSImport("golem:secrets/reveal@0.1.0", JSImport.Namespace)
  private object RevealModule extends js.Object {
    def reveal(secret: JsSecretResource, expected: JsSchemaGraph): JsSchemaValueTree = js.native
  }
}
