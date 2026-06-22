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

import golem.config.{AgentConfigDeclaration, AgentConfigSource}
import golem.host.js._
import golem.runtime._
import golem.runtime.http._

import scala.scalajs.js

/**
 * Builds the schema-native [[AgentTypeEncoderV2.AgentRequest]] surface from an
 * agent's compile-time [[AgentMetadata]] (the live declaration source), then
 * hands it to [[AgentTypeEncoderV2]] to emit the merged `golem:agent@2.0.0`
 * `agent-type`.
 *
 * The agent-type is built from `AgentMetadata` (not from the runtime codecs):
 * the metadata's per-parameter [[ParameterMetadata]] graphs are exactly the v2
 * `input-schema = parameters(named-field…)` shape, whereas a codec's combined
 * record graph would collapse the parameters into one field.
 *
 * HTTP mount validation runs first, description/promptHint fallbacks are
 * applied, and the HTTP endpoint / mount / snapshotting / read-only conversions
 * target the `golem.host.js` facades.
 */
private[autowire] object AgentRequestBuilder {

  def fromMetadata(metadata: AgentMetadata, mode: String): AgentTypeEncoderV2.AgentRequest = {
    // Validate HTTP mount against constructor params — runs lazily when the
    // agent-type is first accessed, so errors surface as AgentError to the host.
    HttpValidation.validateHttpMountFromMetadata(metadata)

    val typeName = metadata.name

    val constructor = AgentTypeEncoderV2.Constructor(
      description = Option(metadata.constructor.description).getOrElse(typeName),
      params = metadata.constructor.input.parameters.map(toParam),
      name = metadata.constructor.name,
      promptHint = metadata.constructor.promptHint
    )

    val methods = metadata.methods.map { m =>
      AgentTypeEncoderV2.Method(
        name = m.name,
        description = m.description.getOrElse(m.name),
        params = m.input.parameters.map(toParam),
        output = m.output match {
          case OutputMetadata.Unit          => None
          case OutputMetadata.Single(graph) => Some(graph)
        },
        httpEndpoints = encodeHttpEndpoints(m.httpEndpoints),
        promptHint = m.prompt,
        readOnly = m.readOnly.fold[js.UndefOr[JsReadOnlyConfig]](js.undefined)(r => encodeReadOnly(r))
      )
    }

    AgentTypeEncoderV2.AgentRequest(
      typeName = typeName,
      description = metadata.description.getOrElse(typeName),
      mode = mode,
      constructor = constructor,
      methods = methods,
      snapshotting = encodeSnapshotting(metadata.snapshotting),
      config = metadata.config.map(toConfigDecl),
      httpMount = metadata.httpMount.fold[js.UndefOr[JsHttpMountDetails]](js.undefined)(m => encodeHttpMount(m))
    )
  }

  private def toParam(p: ParameterMetadata): AgentTypeEncoderV2.Param =
    AgentTypeEncoderV2.Param(
      name = p.name,
      source = p.source match {
        case FieldSource.UserSupplied          => AgentTypeEncoderV2.FieldSource.UserSupplied
        case FieldSource.AutoInjectedPrincipal => AgentTypeEncoderV2.FieldSource.AutoInjectedPrincipal
      },
      graph = p.graph,
      metadata = p.metadata
    )

  private def toConfigDecl(decl: AgentConfigDeclaration): AgentTypeEncoderV2.ConfigDecl = {
    val source = decl.source match {
      case AgentConfigSource.Local  => "local"
      case AgentConfigSource.Secret => "secret"
    }
    AgentTypeEncoderV2.ConfigDecl(source, decl.path, decl.valueType)
  }

  private def encodeHttpMount(mount: HttpMountDetails): JsHttpMountDetails =
    JsHttpMountDetails(
      pathPrefix = encodePathSegments(mount.pathPrefix),
      phantomAgent = mount.phantomAgent,
      corsOptions = JsCorsOptions(js.Array(mount.corsAllowedPatterns: _*)),
      webhookSuffix = encodePathSegments(mount.webhookSuffix),
      authDetails = if (mount.authRequired) JsAuthDetails(required = true) else js.undefined
    )

  private def encodeHttpEndpoints(endpoints: List[HttpEndpointDetails]): js.Array[JsHttpEndpointDetails] = {
    val arr = new js.Array[JsHttpEndpointDetails]()
    endpoints.foreach(ep => arr.push(encodeHttpEndpoint(ep)))
    arr
  }

  private def encodeHttpEndpoint(ep: HttpEndpointDetails): JsHttpEndpointDetails = {
    val headerArr = new js.Array[JsHeaderVariable]()
    ep.headerVars.foreach(h => headerArr.push(JsHeaderVariable(h.headerName, h.variableName)))

    val queryArr = new js.Array[JsQueryVariable]()
    ep.queryVars.foreach(q => queryArr.push(JsQueryVariable(q.queryParamName, q.variableName)))

    val corsOptions = ep.corsOverride match {
      case Some(patterns) => JsCorsOptions(js.Array(patterns: _*))
      case None           => JsCorsOptions(new js.Array[String]())
    }

    val authDetails: js.UndefOr[JsAuthDetails] = ep.authOverride match {
      case Some(required) => JsAuthDetails(required = required)
      case None           => js.undefined
    }

    JsHttpEndpointDetails(
      httpMethod = encodeHttpMethod(ep.httpMethod),
      pathSuffix = encodePathSegments(ep.pathSuffix),
      headerVars = headerArr,
      queryVars = queryArr,
      corsOptions = corsOptions,
      authDetails = authDetails
    )
  }

  private def encodePathSegments(segments: List[PathSegment]): js.Array[JsPathSegment] = {
    val arr = new js.Array[JsPathSegment]()
    segments.foreach {
      case PathSegment.Literal(value)              => arr.push(JsPathSegment.literal(value))
      case PathSegment.PathVariable(name)          => arr.push(JsPathSegment.pathVariable(JsPathVariable(name)))
      case PathSegment.RemainingPathVariable(name) =>
        arr.push(JsPathSegment.remainingPathVariable(JsPathVariable(name)))
      case PathSegment.SystemVariable(name) =>
        arr.push(JsPathSegment.systemVariable(name.asInstanceOf[JsSystemVariable]))
    }
    arr
  }

  private def encodeReadOnly(config: ReadOnlyConfig): JsReadOnlyConfig = {
    val policy = config.cachePolicy match {
      case CachePolicy.NoCache    => JsCachePolicy.noCache
      case CachePolicy.UntilWrite => JsCachePolicy.untilWrite
      case CachePolicy.Ttl(nanos) => JsCachePolicy.ttl(js.BigInt(nanos.toString))
    }
    JsReadOnlyConfig(policy, config.usesPrincipal)
  }

  private def encodeSnapshotting(snapshotting: Snapshotting): JsSnapshotting = snapshotting match {
    case Snapshotting.Disabled        => JsSnapshotting.disabled
    case Snapshotting.Enabled(config) =>
      val jsConfig = config match {
        case SnapshottingConfig.Default         => JsSnapshottingConfig.default
        case SnapshottingConfig.Periodic(nanos) => JsSnapshottingConfig.periodic(js.BigInt(nanos.toString))
        case SnapshottingConfig.EveryN(count)   => JsSnapshottingConfig.everyNInvocation(count)
      }
      JsSnapshotting.enabled(jsConfig)
  }

  private def encodeHttpMethod(method: HttpMethod): JsHttpMethod = method match {
    case HttpMethod.Get            => JsHttpMethod.get
    case HttpMethod.Post           => JsHttpMethod.post
    case HttpMethod.Put            => JsHttpMethod.put
    case HttpMethod.Delete         => JsHttpMethod.delete
    case HttpMethod.Patch          => JsHttpMethod.patch
    case HttpMethod.Head           => JsHttpMethod.head
    case HttpMethod.Options        => JsHttpMethod.options
    case HttpMethod.Connect        => JsHttpMethod.connect
    case HttpMethod.Trace          => JsHttpMethod.trace
    case HttpMethod.Custom(method) => JsHttpMethod.custom(method)
  }
}
