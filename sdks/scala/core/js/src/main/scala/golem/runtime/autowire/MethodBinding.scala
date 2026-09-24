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
import golem.host.SchemaWireInterop
import golem.host.js.schema.{JsAgentError, JsSchemaValueTree}
import golem.runtime.{InputRecordCodec, MethodMetadata, OutputCodec, WireAgentMetadata, WireImplementationMethod}
import golem.runtime.http.HttpMethod
import golem.schema.{
  AgentStream,
  AgentStreamOutputTransaction,
  FromSchema,
  FromSchemaError,
  GuestSchemaValueStreamHandle,
  SchemaValue
}
import golem.schema.SchemaValue.*

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
  def name: String
  def metadata: MethodMetadata

  def invoke(instance: Instance, input: JsSchemaValueTree, principal: Principal): js.Promise[Option[JsSchemaValueTree]]
}

object MethodBinding {
  def wire[Instance](
    descriptor: WireAgentMetadata,
    method: WireImplementationMethod[Instance]
  ): MethodBinding[Instance] =
    new MethodBinding[Instance] {
      def name: String             = method.name
      def metadata: MethodMetadata = descriptor.reflectedMethod(descriptor.methods.find(_.name == name).get)
      def invoke(
        instance: Instance,
        input: JsSchemaValueTree,
        principal: Principal
      ): js.Promise[Option[JsSchemaValueTree]] =
        FutureInterop.toPromise(SchemaPayload.withWireInput(input) { value =>
          method.invoke(instance, value, principal).flatMap {
            case None        => Future.successful(None)
            case Some(value) => SchemaWireInterop.ownedValueTreeToJsAsync(value).map(Some(_))
          }
        })
    }

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
      override val name: String             = methodMetadata.name
      override val metadata: MethodMetadata = methodMetadata
      private val rawHttp                   = metadata.httpEndpoints match {
        case List(endpoint) => endpoint.httpMethod == HttpMethod.Any && endpoint.pathSuffix.isEmpty
        case _              => false
      }
      private val decodedInput = new FromSchema[(In, Boolean)] {
        def fromValue(value: SchemaValue): Either[FromSchemaError, (In, Boolean)] = {
          val head = rawHttp && (value match {
            case RecordValue(List(RecordValue(StringValue("HEAD") :: _))) => true
            case _                                                        => false
          })
          inputCodec.fromValue(value).map(_ -> head)
        }
      }

      override def invoke(
        instance: Instance,
        input: JsSchemaValueTree,
        principal: Principal
      ): js.Promise[Option[JsSchemaValueTree]] = {
        val future = SchemaPayload.withDecodedInput[(In, Boolean), Option[JsSchemaValueTree]](input) {
          case Left(err) =>
            Future.failed(js.JavaScriptException(JsAgentError.invalidInput(err.toString)))
          case Right((value, head)) =>
            handler(instance, value, principal).flatMap { out =>
              outputCodec.into match {
                case None                  => Future.successful(None)
                case Some(into) if rawHttp =>
                  SchemaPayload.encodePreparedValueAsync(into.toValue(out))(suppressBody(_, head)).map(Some(_))
                case Some(into) => SchemaPayload.encodeAsync(out)(into).map(Some(_))
              }
            }
        }(decodedInput)
        FutureInterop.toPromise(future)
      }
    }

  // The component bridge may pull eagerly when wrapping a producer. Dispose forbidden
  // HTTP bodies before that boundary, preserving the host's status/header validation.
  private def suppressBody(value: SchemaValue, head: Boolean): Future[SchemaValue] = value match {
    case RecordValue(List(status @ U16Value(code), headers, StreamValue(handle)))
        if head || code == 204 || code == 205 || code == 304 =>
      val endpoint = handle.take().getOrElse(throw new IllegalStateException("HTTP body was already transferred"))
      AgentStreamOutputTransaction.track(endpoint)
      val empty       = GuestSchemaValueStreamHandle.native(AgentStream.fromPull[SchemaValue](() => Future.successful(None)))
      val replacement = RecordValue(List(status, headers, StreamValue(empty)))
      endpoint.dispose().map(_ => replacement)
    case _ => Future.successful(value)
  }
}
