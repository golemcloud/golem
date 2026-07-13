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

package golem.runtime.macros

import golem.runtime.annotations.*
import golem.schema.{FromSchema, IntoSchema, SchemaValue, TypedSchemaValue}
import golem.tool.*
import zio.ZIO
import zio.test.*

import scala.concurrent.Future

/**
 * Verifies the typed-client call path exactly as the tool RPC codegen emits it
 * (see `ToolRpcCodegen`): the descriptor lookup through
 * [[ToolDefinitionMacro.tryMetadata]], canonical input assembly (static model
 * and inherited-prefix dynamic paths), command path construction, subtree
 * prefix packing, and typed error decoding — all against a fake transport.
 */
object ToolClientRuntimeSpec extends ZIOSpecDefault {

  enum GitError {
    @error(kind = "usage", exitCode = 2)
    case Bad(message: String)
  }

  @toolDefinition(version = "1.0.0")
  trait Git {
    @arg("git-dir", scope = "global")
    def git(gitDir: String): Unit

    def status(short: Boolean): Either[GitError, String]

    @arg("verbose", kind = "flag")
    def remote(verbose: Boolean): Remote
  }

  @toolDefinition(version = "1.0.0")
  trait Remote {
    def add(name: String, url: String): String
  }

  private final class RecordingTransport(
    response: Either[ToolRpcFailure, ToolInvokeResult]
  ) extends ToolRpcTransport {
    var lastCommandPath: List[String]       = Nil
    var lastInput: Option[TypedSchemaValue] = None
    var lastStdin: Option[ToolInputStream]  = None

    def invokeAndAwait(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolInputStream]
    ): Future[Either[ToolRpcFailure, ToolInvokeResult]] = {
      lastCommandPath = commandPath
      lastInput = Some(input)
      lastStdin = stdin
      Future.successful(response)
    }
  }

  // Mirrors the generated companion caches.
  private lazy val gitDescriptor    = ToolDefinitionMacro.tryMetadata[Git]
  private lazy val remoteDescriptor = ToolDefinitionMacro.tryMetadata[Remote]
  private lazy val statusModel      = ToolClientRuntime.staticInputModel(gitDescriptor, List("status"))
  private lazy val gitErrorSchema   = ToolErrorSchemaDerivation.derive[GitError]

  private def ok(text: String): Either[ToolRpcFailure, ToolInvokeResult] =
    Right(ToolInvokeResult(Some(IntoSchema[String].toTyped(text)), None))

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolClientRuntimeSpec")(
      test("root subcommand call assembles the canonical input from the static model") {
        val transport = new RecordingTransport(ok("clean"))
        // Values deliberately listed in non-canonical order to exercise the
        // name-matching fallback (canonical order is [git-dir, short]).
        val params = ToolClientRuntime.encodeParams(
          List(
            ("short", IntoSchema[Boolean].toValue(true)),
            ("git-dir", IntoSchema[String].toValue("/repo"))
          )
        )
        val input = params.flatMap(values => ToolClientRuntime.buildInputFromModel(statusModel, values))
        ZIO
          .fromFuture(_ =>
            ToolClientRuntime.complete(
              ToolClientRuntime
                .run[GitError](transport, List("status"), input, None, gitErrorSchema.fromErrorPayloadValue(_))
            )(r => ToolClientRuntime.decodeValueResult(r, implicitly[FromSchema[String]]))
          )
          .map { result =>
            val record = transport.lastInput.map(_.value)
            assertTrue(
              result == Right("clean"),
              transport.lastCommandPath == List("status"),
              record == Some(
                SchemaValue.RecordValue(
                  List(SchemaValue.StringValue("/repo"), SchemaValue.BoolValue(true))
                )
              )
            )
          }
      },
      test("subtree navigation packs the inherited prefix and calls through the child descriptor") {
        val transport = new RecordingTransport(ok("added"))
        // Mirrors the generated `remote(...)` navigation: inherited global
        // first, then the subtree method's own flag parameter.
        val prefix = List(
          ToolClientRuntime.prefixValue("git-dir", Nil, "/repo", IntoSchema[String]),
          ToolClientRuntime.prefixValue("verbose", Nil, true, IntoSchema[Boolean])
        )
        // Mirrors the generated wrapper leaf `add(...)`: dynamic input against
        // the child's descriptor because a prefix is inherited.
        val params = ToolClientRuntime.encodeParams(
          List(
            ("name", IntoSchema[String].toValue("origin")),
            ("url", IntoSchema[String].toValue("https://example.com"))
          )
        )
        val input =
          params.flatMap(values => ToolClientRuntime.buildDynamicInput(remoteDescriptor, List("add"), prefix, values))
        ZIO
          .fromFuture(_ =>
            ToolClientRuntime.complete(
              ToolClientRuntime.runInfallible(transport, List("remote", "add"), input, None)
            )(r => ToolClientRuntime.decodeValueResult(r, implicitly[FromSchema[String]]))
          )
          .map { result =>
            val record = transport.lastInput.map(_.value)
            assertTrue(
              result == Right("added"),
              transport.lastCommandPath == List("remote", "add"),
              record == Some(
                SchemaValue.RecordValue(
                  List(
                    SchemaValue.StringValue("/repo"),
                    SchemaValue.BoolValue(true),
                    SchemaValue.StringValue("origin"),
                    SchemaValue.StringValue("https://example.com")
                  )
                )
              )
            )
          }
      },
      test("remote custom errors decode into the declared error type") {
        val payload   = gitErrorSchema.toErrorPayloadValue(GitError.Bad("nope")).toOption.get
        val transport = new RecordingTransport(
          Left(ToolRpcFailure.RemoteToolError(ToolInvokeError.Custom(payload)))
        )
        val params = ToolClientRuntime.encodeParams(
          List(
            ("git-dir", IntoSchema[String].toValue("/repo")),
            ("short", IntoSchema[Boolean].toValue(false))
          )
        )
        val input = params.flatMap(values => ToolClientRuntime.buildInputFromModel(statusModel, values))
        ZIO
          .fromFuture(_ =>
            ToolClientRuntime
              .run[GitError](transport, List("status"), input, None, gitErrorSchema.fromErrorPayloadValue(_))
          )
          .map(result => assertTrue(result == Left(ToolError.Tool(GitError.Bad("nope")))))
      },
      test("a missing canonical field surfaces as a protocol error") {
        val transport = new RecordingTransport(ok("unused"))
        val params    = ToolClientRuntime.encodeParams(
          List(("short", IntoSchema[Boolean].toValue(true)))
        )
        val input = params.flatMap(values => ToolClientRuntime.buildInputFromModel(statusModel, values))
        assertTrue(
          input == Left(
            ToolError.Rpc(RpcError.Protocol("missing canonical tool input field `git-dir`"))
          )
        )
      }
    )
}
