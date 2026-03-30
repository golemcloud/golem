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

/**
 * Pure Scala model types for HTTP route metadata. These are cross-compiled (JVM
 * + JS, Scala 2 + 3) and have no JS dependency.
 */

sealed trait PathSegment extends Product with Serializable
object PathSegment {
  final case class Literal(value: String)                      extends PathSegment
  final case class PathVariable(variableName: String)          extends PathSegment
  final case class RemainingPathVariable(variableName: String) extends PathSegment
  final case class SystemVariable(name: String)                extends PathSegment
}

final case class HeaderVariable(headerName: String, variableName: String)
final case class QueryVariable(queryParamName: String, variableName: String)

sealed trait HttpMethod extends Product with Serializable
object HttpMethod {
  case object Get                         extends HttpMethod
  case object Post                        extends HttpMethod
  case object Put                         extends HttpMethod
  case object Delete                      extends HttpMethod
  case object Patch                       extends HttpMethod
  case object Head                        extends HttpMethod
  case object Options                     extends HttpMethod
  case object Connect                     extends HttpMethod
  case object Trace                       extends HttpMethod
  final case class Custom(method: String) extends HttpMethod

  def fromString(method: String): Either[String, HttpMethod] =
    method.toLowerCase match {
      case "get"     => Right(Get)
      case "post"    => Right(Post)
      case "put"     => Right(Put)
      case "delete"  => Right(Delete)
      case "patch"   => Right(Patch)
      case "head"    => Right(Head)
      case "options" => Right(Options)
      case "connect" => Right(Connect)
      case "trace"   => Right(Trace)
      case _         => Right(Custom(method))
    }
}

final case class HttpMountDetails(
  pathPrefix: List[PathSegment],
  authRequired: Boolean,
  phantomAgent: Boolean,
  corsAllowedPatterns: List[String],
  webhookSuffix: List[PathSegment]
)

final case class HttpEndpointDetails(
  httpMethod: HttpMethod,
  pathSuffix: List[PathSegment],
  headerVars: List[HeaderVariable],
  queryVars: List[QueryVariable],
  authOverride: Option[Boolean],
  corsOverride: Option[List[String]]
)
