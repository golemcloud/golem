// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).
package golem.tool

import golem.runtime.http._

final case class ToolHttpMount(pathPrefix: List[PathSegment])

sealed trait CommandHttp extends Product with Serializable
object CommandHttp {
  final case class Prefix(pathSuffix: List[PathSegment]) extends CommandHttp
  final case class Endpoint(
    routeId: Option[String],
    httpMethod: HttpMethod,
    pathSuffix: List[PathSegment],
    queryVars: List[QueryVariable],
    headerVars: List[HeaderVariable],
    responseHeaders: List[FileResponseHeader],
    mode: Option[HttpRouteMode],
    durableStreams: Option[DurableStreamRouteOptions]
  ) extends CommandHttp
}
