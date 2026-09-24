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
import golem.host.SchemaWireInterop
import golem.host.js._
import golem.host.js.schema._
import golem.runtime._
import golem.runtime.http._

import scala.scalajs.js
import scala.scalajs.js.JSConverters._

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

  def fromWire(metadata: WireAgentMetadata, mode: String): JsAgentType = {
    def parameters(values: List[WireParameterMetadata]): JsInputSchema = JsInputSchema.parameters(values.map { p =>
      JsNamedField(
        p.name,
        p.source match {
          case FieldSource.UserSupplied          => JsFieldSource.userSupplied
          case FieldSource.AutoInjectedPrincipal => JsFieldSource.autoInjectedPrincipal
        },
        p.schema,
        SchemaWireInterop.metadataToJs(p.metadata)
      )
    }.toJSArray)
    JsAgentType(
      metadata.name,
      metadata.kind match {
        case AgentTypeKind.Regular    => "regular"
        case AgentTypeKind.HttpRouter => "http-router"
      },
      metadata.description.getOrElse(metadata.name),
      "scala",
      SchemaWireInterop.graphToJs(metadata.schema),
      JsAgentConstructor(
        metadata.constructor.description,
        parameters(metadata.constructor.parameters),
        metadata.constructor.name.orUndefined,
        metadata.constructor.promptHint.orUndefined
      ),
      metadata.methods.map { m =>
        JsAgentMethod(
          m.name,
          m.description.getOrElse(m.name),
          encodeHttpEndpoints(m.httpEndpoints),
          parameters(m.parameters),
          m.output.fold(JsOutputSchema.unit)(JsOutputSchema.single),
          m.prompt.orUndefined,
          m.readOnly.map(encodeReadOnly).orUndefined
        )
      }.toJSArray,
      new js.Array[JsAgentDependency](),
      mode,
      encodeSnapshotting(metadata.snapshotting),
      metadata.config.map { c =>
        JsAgentConfigDeclaration(
          c.source match {
            case AgentConfigSource.Local  => "local"
            case AgentConfigSource.Secret => "secret"
          },
          c.path.toJSArray,
          c.schema
        )
      }.toJSArray,
      metadata.httpMount.map(encodeHttpMount).orUndefined
    )
  }

  def fromMetadata(metadata: AgentMetadata, mode: String): AgentTypeEncoderV2.AgentRequest = {
    HttpAgentValidation.checked(metadata.copy(mode = Some(mode)))
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
      kind = metadata.kind match {
        case AgentTypeKind.Regular    => "regular"
        case AgentTypeKind.HttpRouter => "http-router"
      },
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
      staticBindings = encodeFileMappings(mount.staticBindings),
      filesystemBindings = encodeFileMappings(mount.filesystemBindings),
      openapiProviderMethod = mount.openapiProviderMethod.orUndefined,
      authDetails = if (mount.authRequired) JsAuthDetails(required = true) else js.undefined
    )

  private def encodeFileMappings(mappings: List[FileMapping]): js.Array[JsFileMapping] =
    mappings.map {
      case FileMapping.Exact(publicPath, filePath)           => JsFileMapping.exact(publicPath.toJSArray, filePath)
      case FileMapping.Subtree(publicPrefix, filesystemRoot) =>
        JsFileMapping.subtree(publicPrefix.toJSArray, filesystemRoot)
    }.toJSArray

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
      authDetails = authDetails,
      durableStreams =
        ep.durableStreams.fold[js.UndefOr[JsDurableStreamRouteOptions]](js.undefined)(encodeDurableStreams)
    )
  }

  private def encodeDurableStreams(options: DurableStreamRouteOptions): JsDurableStreamRouteOptions = {
    val slots = js.Array(options.slots.map { slot =>
      val source = slot.source match {
        case DurableStreamSlotSource.Input(name)  => JsDurableStreamSlotSource.input(name)
        case DurableStreamSlotSource.Output(name) => JsDurableStreamSlotSource.output(name)
      }
      JsDurableStreamSlotOptions(source, slot.name.orUndefined, slot.contentType.orUndefined)
    }: _*)
    val load = options.load.fold[js.UndefOr[JsDurableStreamRouteLoadOptions]](js.undefined)(v =>
      JsDurableStreamRouteLoadOptions(
        v.maxConcurrentReadersPerStream.orUndefined,
        v.maxAppendRequestsPerSecondPerStream.orUndefined
      )
    )
    JsDurableStreamRouteOptions(
      slots,
      options.allowExternalWrites.orUndefined,
      options.allowStreamDelete.orUndefined,
      options.allowInvocationDelete.orUndefined,
      load
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
    case HttpMethod.Any            => JsHttpMethod.any
    case HttpMethod.Custom(method) => JsHttpMethod.custom(method)
  }
}
