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

package golem.runtime.tool

import golem.schema.TypedSchemaValue
import golem.tool.*
import zio.test.*

import scala.concurrent.Future

object ToolMiddlewareRegistrySpec extends ZIOSpecDefault {
  import ToolTestFixtures.*

  private final class UniversalPolicy extends UniversalToolMiddleware {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation,
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
      underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
  }

  private def universalHandle(name: String): UniversalToolMiddlewareHandle =
    UniversalToolMiddlewareHandle(
      ToolMiddlewareDescriptor(name, Nil, Doc.empty, ToolMiddlewareScope.Universal),
      () => new UniversalPolicy
    )

  private def monomorphicHandle(
    name: String,
    presented: Option[ExtendedToolType] = None,
    expected: Option[ExtendedToolType] = None,
    descriptorPresented: Option[ExtendedToolType] = None,
    descriptorExpected: Option[ExtendedToolType] = None
  ): MonomorphicToolMiddlewareHandle = {
    val actualPresented = presented.getOrElse(leafTool(s"$name-tool"))
    val actualExpected  = expected.getOrElse(actualPresented)
    val scopePresented  = descriptorPresented.getOrElse(actualPresented)
    val scopeExpected   = descriptorExpected.getOrElse(actualExpected)
    val presentedWire   = scopePresented.tryToTool.toOption.get
    val expectedWire    = scopeExpected.tryToTool.toOption.get
    MonomorphicToolMiddlewareHandle(
      _ =>
        Right(
          ToolMiddlewareDescriptor(
            name,
            Nil,
            Doc.empty,
            ToolMiddlewareScope.Monomorphic(presentedWire, Some(expectedWire))
          )
        ),
      _ => Right(actualPresented),
      _ => Right(actualExpected),
      () => (),
      Nil
    )
  }

  private def registrationFailure(register: => Unit): Option[Throwable] =
    scala.util.Try(register).failed.toOption

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolMiddlewareRegistrySpec")(
      test("universal and monomorphic middleware coexist") {
        ToolMiddlewareRegistry.clearForTests()
        val universal   = universalHandle("registry-universal")
        val monomorphic = monomorphicHandle("registry-monomorphic")
        ToolMiddlewareImplementationRuntime.registerUniversal(universal)
        ToolMiddlewareImplementationRuntime.registerMonomorphic(monomorphic)
        assertTrue(
          ToolMiddlewareRegistry.getInvoker("registry-universal").exists {
            case ToolMiddlewareRegistry.ToolMiddlewareInvoker.Universal(handle) => handle eq universal
            case _                                                              => false
          },
          ToolMiddlewareRegistry.getInvoker("registry-monomorphic").exists {
            case ToolMiddlewareRegistry.ToolMiddlewareInvoker.Monomorphic(_, _, handle) => handle eq monomorphic
            case _                                                                      => false
          }
        )
      },
      test("middleware names remain unique across scopes") {
        ToolMiddlewareRegistry.clearForTests()
        val universal = universalHandle("registry-duplicate")
        ToolMiddlewareImplementationRuntime.registerUniversal(universal)
        val duplicate = registrationFailure(
          ToolMiddlewareImplementationRuntime.registerMonomorphic(monomorphicHandle("registry-duplicate"))
        )
        assertTrue(
          duplicate.exists(_.isInstanceOf[IllegalArgumentException]),
          duplicate.exists(_.getMessage.contains("registry-duplicate"))
        )
      },
      test("discovery and lookup use canonical names and sorted descriptors") {
        ToolMiddlewareRegistry.clearForTests()
        ToolMiddlewareImplementationRuntime.registerUniversal(universalHandle("registry-zz-last"))
        ToolMiddlewareImplementationRuntime.registerMonomorphic(monomorphicHandle("registry-aa-first"))
        val names = ToolMiddlewareRegistry.allMiddlewares.map(_.name)
        assertTrue(
          names == List("registry-aa-first", "registry-zz-last"),
          ToolMiddlewareRegistry.getMiddleware("registry-aa-first").exists(_.name == "registry-aa-first"),
          ToolMiddlewareRegistry.getMiddleware("registry-missing").isEmpty,
          ToolMiddlewareRegistry.getInvoker("registry-missing").isEmpty
        )
      },
      test("tool and middleware namespaces are independent") {
        ToolMiddlewareRegistry.clearForTests()
        ToolRegistry.clearForTests()
        ToolRegistry.register(leafTool("registry-shared"))
        ToolMiddlewareImplementationRuntime.registerUniversal(universalHandle("registry-shared"))
        assertTrue(
          ToolRegistry.getTool("registry-shared").nonEmpty,
          ToolMiddlewareRegistry.getMiddleware("registry-shared").nonEmpty
        )
      },
      test("registration validates names, tool encoding, and descriptor scope") {
        ToolMiddlewareRegistry.clearForTests()
        val emptyName = registrationFailure(
          ToolMiddlewareImplementationRuntime.registerUniversal(universalHandle(" "))
        )
        val invalidPresented = registrationFailure(
          ToolMiddlewareImplementationRuntime.registerMonomorphic(
            monomorphicHandle(
              "registry-invalid-presented",
              presented = Some(leafTool("Registry-Invalid")),
              expected = Some(leafTool("registry-valid-expected")),
              descriptorPresented = Some(leafTool("registry-valid-descriptor")),
              descriptorExpected = Some(leafTool("registry-valid-expected"))
            )
          )
        )
        val mismatchedScope = registrationFailure(
          ToolMiddlewareImplementationRuntime.registerMonomorphic(
            monomorphicHandle(
              "registry-mismatched-scope",
              descriptorPresented = Some(leafTool("registry-other-presented"))
            )
          )
        )
        val universalWithMonomorphicScope = {
          val tool = leafTool("registry-universal-wrong-scope").tryToTool.toOption.get
          registrationFailure(
            ToolMiddlewareImplementationRuntime.registerUniversal(
              UniversalToolMiddlewareHandle(
                ToolMiddlewareDescriptor(
                  "registry-universal-wrong-scope",
                  Nil,
                  Doc.empty,
                  ToolMiddlewareScope.Monomorphic(tool, Some(tool))
                ),
                () => new UniversalPolicy
              )
            )
          )
        }
        assertTrue(
          emptyName.exists(_.getMessage.contains("must not be empty")),
          invalidPresented.exists(_.getMessage.contains("presented descriptor build failed")),
          mismatchedScope.exists(_.getMessage.contains("does not match")),
          universalWithMonomorphicScope.exists(_.getMessage.contains("universal descriptor scope")),
          ToolMiddlewareRegistry.allMiddlewares.isEmpty
        )
      }
    ) @@ TestAspect.sequential
}
