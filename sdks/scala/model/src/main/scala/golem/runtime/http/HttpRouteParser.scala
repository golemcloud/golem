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
 * Parser for HTTP route templates like
 * "/api/{city}/weather?q={query}&limit={n}"
 */
object HttpRouteParser {

  final case class ParsedRouteTemplate(
    pathSegments: List[PathSegment],
    queryVars: List[QueryVariable]
  )

  def parse(template: String): Either[String, ParsedRouteTemplate] = {
    if (template.isEmpty) return Left("Route template must not be empty")

    val (pathPart, queryPart) = template.indexOf('?') match {
      case -1  => (template, None)
      case idx => (template.substring(0, idx), Some(template.substring(idx + 1)))
    }

    for {
      segments <- parsePath(pathPart)
      queries  <- queryPart.fold[Either[String, List[QueryVariable]]](Right(Nil))(parseQuery)
    } yield ParsedRouteTemplate(segments, queries)
  }

  def parsePath(path: String): Either[String, List[PathSegment]] = {
    if (!path.startsWith("/")) return Left("HTTP path must start with \"/\"")
    if (path == "/") return Right(Nil)

    val segments = path.split("/", -1).drop(1) // drop leading empty string from leading /
    val result   = new scala.collection.mutable.ListBuffer[PathSegment]()
    var i        = 0
    while (i < segments.length) {
      val segment = segments(i)
      val isLast  = i == segments.length - 1

      if (segment.isEmpty) return Left("Empty path segment (\"//\") is not allowed")
      if (segment != segment.trim) return Left("Whitespace is not allowed in path segments")

      if (segment.startsWith("{") && segment.endsWith("}")) {
        val name = segment.substring(1, segment.length - 1)
        if (name.isEmpty) return Left("Empty path variable \"{}\" is not allowed")

        if (name.startsWith("*")) {
          if (!isLast)
            return Left(s"Remaining path variable \"{$name}\" is only allowed as the last path segment")
          val varName = name.substring(1)
          if (varName.isEmpty) return Left("Remaining path variable name cannot be empty")
          result += PathSegment.RemainingPathVariable(varName)
        } else if (name == "agent-type" || name == "agent-version") {
          result += PathSegment.SystemVariable(name)
        } else {
          if (name.isEmpty) return Left("Path variable name cannot be empty")
          result += PathSegment.PathVariable(name)
        }
      } else {
        if (segment.contains("{") || segment.contains("}"))
          return Left(
            s"Path segment \"$segment\" must be a whole variable like \"{id}\" and cannot mix literals and variables"
          )
        result += PathSegment.Literal(segment)
      }
      i += 1
    }
    Right(result.toList)
  }

  def parseQuery(query: String): Either[String, List[QueryVariable]] = {
    if (query.isEmpty) return Right(Nil)

    val pairs  = query.split("&")
    val result = new scala.collection.mutable.ListBuffer[QueryVariable]()
    var i      = 0
    while (i < pairs.length) {
      val pair  = pairs(i)
      val parts = pair.split("=", 2)
      if (parts.length != 2 || parts(0).isEmpty || parts(1).isEmpty)
        return Left(s"Invalid query segment \"$pair\"")

      val key   = parts(0)
      val value = parts(1)

      if (value != value.trim) return Left("Whitespace is not allowed in query variables")
      if (!value.startsWith("{") || !value.endsWith("}"))
        return Left(s"Query value for \"$key\" must be a variable reference like \"{varName}\"")

      val varName = value.substring(1, value.length - 1)
      if (varName.isEmpty) return Left("Query variable name cannot be empty")

      result += QueryVariable(key, varName)
      i += 1
    }
    Right(result.toList)
  }

  /**
   * Parse a path-only template, rejecting any query parameters. Used for mount
   * paths and webhook suffixes.
   */
  def parsePathOnly(template: String, entityName: String): Either[String, List[PathSegment]] =
    if (template.contains("?"))
      Left(s"$entityName must not contain query parameters")
    else
      parsePath(template)
}
