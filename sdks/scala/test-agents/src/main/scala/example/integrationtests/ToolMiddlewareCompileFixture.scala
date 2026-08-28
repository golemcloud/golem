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

package example.integrationtests

import golem.Principal
import golem.runtime.annotations.{arg, error, toolDefinition, toolMiddleware, universalToolMiddleware}
import golem.schema.TypedSchemaValue
import golem.tool.{
  ToolInvokeError,
  ToolInvokeResult,
  UniversalToolMiddleware,
  UniversalToolMiddlewareInvocation,
  UniversalToolUnderlying
}

import scala.concurrent.Future

enum MiddlewareFixtureError {
  @error(kind = "usage", exitCode = 2)
  case Rejected(message: String)
}

enum MiddlewareFixtureBackendError {
  @error(kind = "runtime", exitCode = 1)
  case Failed(message: String)
}

@toolDefinition(name = "middleware-fixture", version = "1.0.0")
trait MiddlewareFixtureTool {
  @arg("config", scope = "global")
  def middlewareFixture(config: String): Unit

  def call(value: String, principal: Principal): Either[MiddlewareFixtureError, String]

  def nested(prefix: String): MiddlewareFixtureNested
}

@toolDefinition(name = "nested", version = "1.0.0")
trait MiddlewareFixtureNested {
  def inspect(name: String): String
}

@toolDefinition(name = "middleware-fixture-backend", version = "1.0.0")
trait MiddlewareFixtureBackend {
  def execute(value: String): Either[MiddlewareFixtureBackendError, Long]
}

@toolMiddleware(name = "middleware-fixture-transparent")
final class MiddlewareFixtureTransparent extends MiddlewareFixtureToolMiddleware {
  def middlewareFixture(
    underlying: MiddlewareFixtureToolUnderlying,
    config: String
  ): Future[Either[ToolInvokeError[Nothing], Unit]] =
    underlying.middlewareFixture(config)

  def call(
    underlying: MiddlewareFixtureToolUnderlying,
    config: String,
    value: String,
    principal: Principal
  ): Future[Either[ToolInvokeError[MiddlewareFixtureError], String]] =
    underlying.call(config, value)

  def inspect(
    underlying: MiddlewareFixtureToolUnderlying,
    config: String,
    prefix: String,
    name: String
  ): Future[Either[ToolInvokeError[Nothing], String]] =
    underlying.inspect(config, prefix, name)
}

@toolMiddleware(name = "middleware-fixture-adapter")
final class MiddlewareFixtureAdapter
    extends MiddlewareFixtureToolMiddleware.Adapter[MiddlewareFixtureBackendUnderlying] {
  def middlewareFixture(
    underlying: MiddlewareFixtureBackendUnderlying,
    config: String
  ): Future[Either[ToolInvokeError[Nothing], Unit]] =
    Future.successful(Right(()))

  def call(
    underlying: MiddlewareFixtureBackendUnderlying,
    config: String,
    value: String,
    principal: Principal
  ): Future[Either[ToolInvokeError[MiddlewareFixtureError], String]] =
    underlying
      .execute(value)
      .map {
        case Right(length) => Right(s"$config:$length")
        case Left(error)   =>
          Left(error.mapTool { case MiddlewareFixtureBackendError.Failed(message) =>
            MiddlewareFixtureError.Rejected(message)
          })
      }(scala.concurrent.ExecutionContext.parasitic)

  def inspect(
    underlying: MiddlewareFixtureBackendUnderlying,
    config: String,
    prefix: String,
    name: String
  ): Future[Either[ToolInvokeError[Nothing], String]] =
    Future.successful(Right(s"$config:$prefix:$name"))
}

@universalToolMiddleware(name = "middleware-fixture-universal")
final class MiddlewareFixtureUniversal extends UniversalToolMiddleware {
  def invoke(
    invocation: UniversalToolMiddlewareInvocation,
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
}
