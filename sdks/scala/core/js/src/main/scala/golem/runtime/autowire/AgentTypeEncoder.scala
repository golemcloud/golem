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

package golem.runtime.autowire

import golem.config.{AgentConfigDeclaration, AgentConfigSource}
import golem.data.StructuredSchema
import golem.host.js._
import golem.runtime._
import golem.runtime.http._

import scala.scalajs.js

object AgentTypeEncoder {
  def from[Instance](definition: AgentDefinition[Instance]): JsAgentType = {
    // Validate HTTP mount against constructor params — runs lazily when
    // agentType is first accessed (inside discoverAgentTypes), so errors
    // are surfaced as AgentError to the Golem host.
    HttpValidation.validateHttpMountFromMetadata(definition.metadata)

    val constructorMeta =
      Option(definition.constructor)
        .map(_.info)
        .getOrElse(ConstructorMetadata(name = None, description = definition.typeName, promptHint = None))

    val idSchema: JsDataSchema =
      Option(definition.constructor)
        .map(_.schema)
        .getOrElse(JsDataSchema.tuple(new js.Array[js.Tuple2[String, JsElementSchema]]()))

    val constructorInfo = JsAgentConstructorDef(
      description = constructorMeta.description,
      inputSchema = idSchema,
      name = Option(constructorMeta.name).flatten.fold[js.UndefOr[String]](js.undefined)(n => n),
      promptHint = Option(constructorMeta.promptHint).flatten.fold[js.UndefOr[String]](js.undefined)(p => p)
    )

    val methodsArray   = new js.Array[JsAgentMethod]()
    val methodBindings = Option(definition.methodMetadata).getOrElse(Nil)
    methodBindings.foreach { binding =>
      if (binding != null) {
        val metadata          = binding.metadata
        val methodDescription = Option(metadata.description).flatten.getOrElse(metadata.name)
        val endpoints         = encodeHttpEndpoints(metadata.httpEndpoints)
        val methodInfo        = JsAgentMethod(
          name = metadata.name,
          description = methodDescription,
          httpEndpoint = endpoints,
          inputSchema = HostSchemaEncoder.encode(binding.metadata.input),
          outputSchema = HostSchemaEncoder.encode(binding.metadata.output),
          promptHint = Option(metadata.prompt).flatten.fold[js.UndefOr[String]](js.undefined)(p => p)
        )
        methodsArray.push(methodInfo)
      }
    }

    val metadataInfo =
      Option(definition.metadata)
        .getOrElse(
          AgentMetadata(definition.typeName, None, Some(definition.mode.value), Nil, StructuredSchema.Tuple(Nil))
        )

    val typeDescription =
      Option(metadataInfo.description).flatten.getOrElse(definition.typeName)

    val jsHttpMount: js.UndefOr[JsHttpMountDetails] =
      metadataInfo.httpMount.fold[js.UndefOr[JsHttpMountDetails]](js.undefined)(m => encodeHttpMount(m))

    JsAgentType(
      typeName = definition.typeName,
      description = typeDescription,
      sourceLanguage = "scala",
      constructor = constructorInfo,
      methods = methodsArray,
      dependencies = new js.Array[JsAgentDependency](),
      mode = definition.mode.value,
      snapshotting = encodeSnapshotting(metadataInfo.snapshotting),
      config = encodeConfigDeclarations(metadataInfo.config),
      httpMount = jsHttpMount
    )
  }

  private def encodeConfigDeclarations(decls: List[AgentConfigDeclaration]): js.Array[JsAgentConfigDeclaration] = {
    val arr = new js.Array[JsAgentConfigDeclaration]()
    decls.foreach { decl =>
      val source: JsAgentConfigSource = decl.source match {
        case AgentConfigSource.Local  => "local"
        case AgentConfigSource.Secret => "secret"
      }
      val path    = js.Array(decl.path: _*)
      val witType = decl.valueType match {
        case golem.data.ElementSchema.Component(dataType) =>
          WitTypeBuilder.build(dataType)
        case _ =>
          throw new UnsupportedOperationException(s"Config declaration only supports component schemas")
      }
      arr.push(JsAgentConfigDeclaration(source, path, witType))
    }
    arr
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
