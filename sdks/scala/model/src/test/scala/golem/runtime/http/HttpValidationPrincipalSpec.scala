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

package golem.runtime.http

import zio.test._

object HttpValidationPrincipalSpec extends ZIOSpecDefault {

  private val principalParams = Set("caller")

  def spec = suite("HttpValidationPrincipalSpec")(
    suite("endpoint Principal validation")(
      test("rejects path variable referencing Principal param") {
        val endpoint = HttpEndpointDetails(
          httpMethod = HttpMethod.Get,
          pathSuffix = List(PathSegment.PathVariable("caller")),
          headerVars = Nil,
          queryVars = Nil,
          authOverride = None,
          corsOverride = None
        )
        val result = HttpValidation.validateEndpointVars(
          "TestAgent",
          "myMethod",
          endpoint,
          Set("caller", "name"),
          principalParams,
          hasMount = true
        )
        assertTrue(
          result.isLeft,
          result.left.toOption.get.contains("Principal-typed parameter")
        )
      },
      test("rejects header variable referencing Principal param") {
        val endpoint = HttpEndpointDetails(
          httpMethod = HttpMethod.Post,
          pathSuffix = List(PathSegment.Literal("action")),
          headerVars = List(HeaderVariable("X-Caller", "caller")),
          queryVars = Nil,
          authOverride = None,
          corsOverride = None
        )
        val result = HttpValidation.validateEndpointVars(
          "TestAgent",
          "myMethod",
          endpoint,
          Set("caller", "name"),
          principalParams,
          hasMount = true
        )
        assertTrue(
          result.isLeft,
          result.left.toOption.get.contains("Principal-typed parameter")
        )
      },
      test("rejects query variable referencing Principal param") {
        val endpoint = HttpEndpointDetails(
          httpMethod = HttpMethod.Get,
          pathSuffix = Nil,
          headerVars = Nil,
          queryVars = List(QueryVariable("caller_id", "caller")),
          authOverride = None,
          corsOverride = None
        )
        val result = HttpValidation.validateEndpointVars(
          "TestAgent",
          "myMethod",
          endpoint,
          Set("caller", "name"),
          principalParams,
          hasMount = true
        )
        assertTrue(
          result.isLeft,
          result.left.toOption.get.contains("Principal-typed parameter")
        )
      },
      test("accepts endpoint with non-Principal params") {
        val endpoint = HttpEndpointDetails(
          httpMethod = HttpMethod.Get,
          pathSuffix = List(PathSegment.PathVariable("name")),
          headerVars = Nil,
          queryVars = Nil,
          authOverride = None,
          corsOverride = None
        )
        val result = HttpValidation.validateEndpointVars(
          "TestAgent",
          "myMethod",
          endpoint,
          Set("caller", "name"),
          principalParams,
          hasMount = true
        )
        assertTrue(result.isRight)
      },
      test("accepts endpoint when principalParamNames is empty") {
        val endpoint = HttpEndpointDetails(
          httpMethod = HttpMethod.Get,
          pathSuffix = List(PathSegment.PathVariable("caller")),
          headerVars = Nil,
          queryVars = Nil,
          authOverride = None,
          corsOverride = None
        )
        val result = HttpValidation.validateEndpointVars(
          "TestAgent",
          "myMethod",
          endpoint,
          Set("caller"),
          Set.empty,
          hasMount = true
        )
        assertTrue(result.isRight)
      },
      test("rejects remaining path variable referencing Principal param") {
        val endpoint = HttpEndpointDetails(
          httpMethod = HttpMethod.Get,
          pathSuffix = List(PathSegment.RemainingPathVariable("caller")),
          headerVars = Nil,
          queryVars = Nil,
          authOverride = None,
          corsOverride = None
        )
        val result = HttpValidation.validateEndpointVars(
          "TestAgent",
          "myMethod",
          endpoint,
          Set("caller"),
          principalParams,
          hasMount = true
        )
        assertTrue(
          result.isLeft,
          result.left.toOption.get.contains("Principal-typed parameter")
        )
      }
    ),
    suite("mount Principal validation")(
      test("rejects mount path variable referencing Principal param") {
        val mount = HttpMountDetails(
          pathPrefix = List(PathSegment.Literal("api"), PathSegment.PathVariable("caller")),
          authRequired = false,
          phantomAgent = false,
          corsAllowedPatterns = Nil,
          webhookSuffix = Nil
        )
        val result = HttpValidation.validateMountVarsAreNotPrincipal("TestAgent", mount, principalParams)
        assertTrue(
          result.isLeft,
          result.left.toOption.get.contains("Principal-typed constructor parameter")
        )
      },
      test("accepts mount with non-Principal path variables") {
        val mount = HttpMountDetails(
          pathPrefix = List(PathSegment.Literal("api"), PathSegment.PathVariable("name")),
          authRequired = false,
          phantomAgent = false,
          corsAllowedPatterns = Nil,
          webhookSuffix = Nil
        )
        val result = HttpValidation.validateMountVarsAreNotPrincipal("TestAgent", mount, principalParams)
        assertTrue(result.isRight)
      },
      test("accepts mount when principalParamNames is empty") {
        val mount = HttpMountDetails(
          pathPrefix = List(PathSegment.Literal("api"), PathSegment.PathVariable("caller")),
          authRequired = false,
          phantomAgent = false,
          corsAllowedPatterns = Nil,
          webhookSuffix = Nil
        )
        val result = HttpValidation.validateMountVarsAreNotPrincipal("TestAgent", mount, Set.empty)
        assertTrue(result.isRight)
      }
    )
  )
}
