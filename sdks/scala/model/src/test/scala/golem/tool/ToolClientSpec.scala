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

import golem.schema.{FromSchema, IntoSchema, TypedSchemaValue}
import zio.ZIO
import zio.test._

import scala.concurrent.Future

/**
 * The Scala port of the Rust SDK's `tool_client.rs` unit tests: custom-error
 * payload decoding, custom-error decode through `invokeAndAwait`, and framing
 * error mapping, all against fake transports.
 */
object ToolClientSpec extends ZIOSpecDefault {

  private sealed trait CliError                   extends Product with Serializable
  private final case class Usage(message: String) extends CliError

  private def decodeCliError(value: TypedSchemaValue): Either[String, CliError] =
    implicitly[FromSchema[String]]
      .fromValue(value.value)
      .map(Usage(_): CliError)
      .left
      .map(e => s"failed to decode remote tool error: ${e.message}")

  private val unitInput: TypedSchemaValue = ToolErrorSupport.unitPayload

  private def stringPayload(text: String): TypedSchemaValue =
    implicitly[IntoSchema[String]].toTyped(text)

  private final class FakeToolRpc extends ToolRpcTransport {
    def invokeAndAwait(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolInputStream]
    ): Future[Either[ToolRpcFailure, ToolInvokeResult]] =
      Future.successful(
        Left(ToolRpcFailure.RemoteToolError(ToolInvokeError.Custom(stringPayload("bad flag"))))
      )
  }

  private sealed trait FakeFailure
  private object FakeFailure {
    case object Denied             extends FakeFailure
    case object RemoteInvalidInput extends FakeFailure
  }

  private final class FailingToolRpc(failure: FakeFailure) extends ToolRpcTransport {
    def invokeAndAwait(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolInputStream]
    ): Future[Either[ToolRpcFailure, ToolInvokeResult]] =
      Future.successful(Left(failure match {
        case FakeFailure.Denied             => ToolRpcFailure.Denied("no access")
        case FakeFailure.RemoteInvalidInput =>
          ToolRpcFailure.RemoteToolError(ToolInvokeError.InvalidInput("bad wire input"))
      }))
  }

  def spec: Spec[Any, Any] = suite("ToolClientSpec")(
    test("custom_tool_error_payload_decodes_to_declared_error_variant") {
      val decoded = ToolClientRuntime.mapRemoteToolError(
        ToolInvokeError.Custom(stringPayload("bad flag")),
        decodeCliError
      )
      assertTrue(decoded == ToolError.Tool(Usage("bad flag")))
    },
    test("invoke_and_await_decoding_error_decodes_custom_tool_error_payload") {
      ZIO
        .fromFuture(_ => ToolClientRuntime.invokeAndAwait(new FakeToolRpc, Nil, unitInput, None, decodeCliError))
        .map {
          case Left(ToolError.Tool(Usage(message))) => assertTrue(message == "bad flag")
          case other                                => assertNever(s"expected declared tool error, got: $other")
        }
    },
    test("invoke_and_await_maps_framing_errors_to_rpc_errors") {
      for {
        denied <- ZIO.fromFuture(_ =>
                    ToolClientRuntime.invokeAndAwaitPayloadError[String](
                      new FailingToolRpc(FakeFailure.Denied),
                      Nil,
                      unitInput,
                      None
                    )
                  )
        remote <- ZIO.fromFuture(_ =>
                    ToolClientRuntime.invokeAndAwaitPayloadError[String](
                      new FailingToolRpc(FakeFailure.RemoteInvalidInput),
                      Nil,
                      unitInput,
                      None
                    )
                  )
      } yield {
        val deniedOk = denied match {
          case Left(ToolError.Rpc(RpcError.Denied(message))) => message == "no access"
          case _                                             => false
        }
        val remoteOk = remote match {
          case Left(ToolError.Rpc(RpcError.Protocol(message))) =>
            message.contains("remote tool error: invalid input: bad wire input")
          case _ => false
        }
        assertTrue(deniedOk, remoteOk)
      }
    }
  )
}
