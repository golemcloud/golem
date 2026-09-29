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

package golem.tool

import golem.Principal
import golem.schema.SchemaDecodeError
import golem.schema.wire.*
import golem.tool.wire.*

import scala.concurrent.{ExecutionContext, Future}
import scala.util.control.NonFatal

final case class WireToolInput(
  value: WitSchemaValueTree,
  stdin: Option[ToolInputStream],
  stdout: Option[ToolOutputStream],
  principal: Principal
)

final case class WireToolBinding(
  paths: List[List[String]],
  stdout: Option[StreamSpec],
  invoke: WireToolInput => Future[Either[WitToolError, Option[WitTypedSchemaValue]]]
)

private final case class WireInputFailure(message: String) extends RuntimeException(message)

final case class WireToolImplementation(descriptor: WitTool, bindings: List[WireToolBinding]) {
  private val dispatch = bindings.flatMap(binding => binding.paths.map(_ -> binding)).toMap

  def invoke(path: List[String], input: WireToolInput): Future[Either[WitToolError, Option[WitTypedSchemaValue]]] =
    dispatch.get(path) match {
      case Some(binding) if binding.stdout.isEmpty && input.stdout.isDefined =>
        reject(input, WitToolError.InvalidInput("unexpected stdout stream"))
      case Some(binding) if binding.stdout.exists(_.required) && input.stdout.isEmpty =>
        reject(input, WitToolError.InvalidInput("tool invocation did not contain declared stdout stream"))
      case Some(binding) =>
        try binding.invoke(input)
        catch {
          case error: WireInputFailure => reject(input, WitToolError.InvalidInput(error.message))
        }
      case None => reject(input, WitToolError.InvalidCommandPath(path))
    }

  private def reject(
    input: WireToolInput,
    error: WitToolError
  ): Future[Either[WitToolError, Option[WitTypedSchemaValue]]] = {
    new WireValuesReader(input.value).abort()
    Future.successful(Left(error))
  }
}

object WireToolImplementation {
  val executionContext: ExecutionContext = ExecutionContext.parasitic

  def arguments(input: WireToolInput, size: Int)(
    decode: (WireValuesReader, Vector[Int]) => Vector[Any]
  ): Vector[Any] = {
    val reader = new WireValuesReader(input.value)
    try {
      val result = reader.at(input.value.root) {
        case WitSchemaValueNode.RecordValue(fields) if fields.length == size => decode(reader, fields)
      }
      reader.finish()
      result
    } catch {
      case NonFatal(error) =>
        reader.abort()
        throw WireInputFailure(String.valueOf(error.getMessage))
    }
  }

  def success[A](
    value: A,
    codec: ConcreteCodec[A],
    graph: WitSchemaGraph
  ): Either[WitToolError, Option[WitTypedSchemaValue]] =
    try Right(Some(WitTypedSchemaValue(graph, codec.encodeValue(value))))
    catch { case error: Throwable => Left(WitToolError.InvalidResult(String.valueOf(error.getMessage))) }

  def custom[A](
    name: String,
    value: A,
    codec: ConcreteCodec[A],
    graph: WitSchemaGraph
  ): Either[WitToolError, Option[WitTypedSchemaValue]] =
    try Left(WitToolError.CustomError(WitCustomToolError(name, WitTypedSchemaValue(graph, codec.encodeValue(value)))))
    catch { case error: Throwable => Left(WitToolError.InvalidResult(String.valueOf(error.getMessage))) }
}
