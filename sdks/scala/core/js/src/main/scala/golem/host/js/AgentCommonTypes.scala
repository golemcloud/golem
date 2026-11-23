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

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// golem:agent/common@2.0.0  –  JS facade traits
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// HTTP types
// ---------------------------------------------------------------------------

@js.native
sealed trait JsHttpMethod extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsConcreteHttpMethod extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsHttpMethodCustom extends JsConcreteHttpMethod {
  @JSName("val") def value: String = js.native
}

object JsHttpMethod {
  private def standard(tag: String): JsHttpMethod =
    JsShape.tagged[JsHttpMethod]("method", JsShape.tagOnly[JsConcreteHttpMethod](tag))

  def get: JsHttpMethod     = standard("get")
  def head: JsHttpMethod    = standard("head")
  def post: JsHttpMethod    = standard("post")
  def put: JsHttpMethod     = standard("put")
  def delete: JsHttpMethod  = standard("delete")
  def connect: JsHttpMethod = standard("connect")
  def options: JsHttpMethod = standard("options")
  def trace: JsHttpMethod   = standard("trace")
  def patch: JsHttpMethod   = standard("patch")
  def any: JsHttpMethod     = JsShape.tagOnly[JsHttpMethod]("any")

  def custom(method: String): JsHttpMethod =
    JsShape.tagged[JsHttpMethod]("method", JsShape.tagged[JsConcreteHttpMethod]("custom", method))
}

@js.native
sealed trait JsFileMapping extends js.Object {
  def tag: String = js.native
}

object JsFileMapping {
  def exact(publicPath: js.Array[String], filePath: String): JsFileMapping =
    JsShape.tagged[JsFileMapping](
      "exact",
      js.Dynamic.literal("publicPath" -> publicPath, "filePath" -> filePath)
    )

  def subtree(publicPrefix: js.Array[String], filesystemRoot: String): JsFileMapping =
    JsShape.tagged[JsFileMapping](
      "subtree",
      js.Dynamic.literal("publicPrefix" -> publicPrefix, "filesystemRoot" -> filesystemRoot)
    )
}

@js.native
sealed trait JsPathVariable extends js.Object {
  def variableName: String = js.native
}

object JsPathVariable {
  def apply(variableName: String): JsPathVariable =
    js.Dynamic.literal("variableName" -> variableName).asInstanceOf[JsPathVariable]
}

@js.native
sealed trait JsPathSegment extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsNeutralPathSegment extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsPathSegmentLiteral extends JsNeutralPathSegment {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsPathSegmentSystemVariable extends JsPathSegment {
  @JSName("val") def value: JsSystemVariable = js.native
}

@js.native
sealed trait JsPathSegmentPathVariable extends JsNeutralPathSegment {
  @JSName("val") def value: JsPathVariable = js.native
}

@js.native
sealed trait JsPathSegmentRemainingPathVariable extends JsNeutralPathSegment {
  @JSName("val") def value: JsPathVariable = js.native
}

object JsPathSegment {
  def literal(value: String): JsPathSegment =
    JsShape.tagged[JsPathSegment]("segment", JsShape.tagged[JsNeutralPathSegment]("literal", value))

  def systemVariable(value: JsSystemVariable): JsPathSegment =
    JsShape.tagged[JsPathSegment]("system-variable", value.asInstanceOf[js.Any])

  def pathVariable(value: JsPathVariable): JsPathSegment =
    JsShape.tagged[JsPathSegment]("segment", JsShape.tagged[JsNeutralPathSegment]("path-variable", value))

  def remainingPathVariable(value: JsPathVariable): JsPathSegment =
    JsShape.tagged[JsPathSegment]("segment", JsShape.tagged[JsNeutralPathSegment]("remaining-path-variable", value))
}

@js.native
sealed trait JsHeaderVariable extends js.Object {
  def headerName: String   = js.native
  def variableName: String = js.native
}

object JsHeaderVariable {
  def apply(headerName: String, variableName: String): JsHeaderVariable =
    js.Dynamic.literal("headerName" -> headerName, "variableName" -> variableName).asInstanceOf[JsHeaderVariable]
}

@js.native
sealed trait JsQueryVariable extends js.Object {
  def queryParamName: String = js.native
  def variableName: String   = js.native
}

object JsQueryVariable {
  def apply(queryParamName: String, variableName: String): JsQueryVariable =
    js.Dynamic.literal("queryParamName" -> queryParamName, "variableName" -> variableName).asInstanceOf[JsQueryVariable]
}

@js.native
sealed trait JsAuthDetails extends js.Object {
  def required: Boolean = js.native
}

object JsAuthDetails {
  def apply(required: Boolean): JsAuthDetails =
    js.Dynamic.literal("required" -> required).asInstanceOf[JsAuthDetails]
}

@js.native
sealed trait JsCorsOptions extends js.Object {
  def allowedPatterns: js.Array[String] = js.native
}

object JsCorsOptions {
  def apply(allowedPatterns: js.Array[String]): JsCorsOptions =
    js.Dynamic.literal("allowedPatterns" -> allowedPatterns).asInstanceOf[JsCorsOptions]
}

@js.native
sealed trait JsAgentIdBinding extends js.Object

object JsAgentIdBinding {
  def path(name: String): JsAgentIdBinding                            = binding("path", name)
  def query(name: String): JsAgentIdBinding                           = binding("query", name)
  private def binding(source: String, name: String): JsAgentIdBinding =
    JsShape.tagged[JsAgentIdBinding](source, js.Dynamic.literal("name" -> name))
}

@js.native
sealed trait JsFileResponseHeader extends js.Object {
  def name: String  = js.native
  def value: String = js.native
}

object JsFileResponseHeader {
  def apply(name: String, value: String): JsFileResponseHeader =
    js.Dynamic.literal("name" -> name, "value" -> value).asInstanceOf[JsFileResponseHeader]
}

@js.native
sealed trait JsHttpMountDetails extends js.Object {
  def agentIdBinding: js.UndefOr[JsAgentIdBinding]        = js.native
  def createRoute: js.UndefOr[js.Array[JsPathSegment]]    = js.native
  def pathPrefix: js.Array[JsPathSegment]                 = js.native
  def authDetails: js.UndefOr[JsAuthDetails]              = js.native
  def phantomAgent: Boolean                               = js.native
  def corsOptions: JsCorsOptions                          = js.native
  def webhookSuffix: js.Array[JsPathSegment]              = js.native
  def staticBindings: js.Array[JsFileMapping]             = js.native
  def filesystemBindings: js.Array[JsFileMapping]         = js.native
  def openapiProviderMethod: js.UndefOr[String]           = js.native
  def fileResponseHeaders: js.Array[JsFileResponseHeader] = js.native
}

object JsHttpMountDetails {
  def apply(
    pathPrefix: js.Array[JsPathSegment],
    phantomAgent: Boolean,
    corsOptions: JsCorsOptions,
    webhookSuffix: js.Array[JsPathSegment],
    staticBindings: js.Array[JsFileMapping],
    filesystemBindings: js.Array[JsFileMapping],
    fileResponseHeaders: js.Array[JsFileResponseHeader],
    openapiProviderMethod: js.UndefOr[String],
    authDetails: js.UndefOr[JsAuthDetails] = js.undefined,
    agentIdBinding: js.UndefOr[JsAgentIdBinding] = js.undefined,
    createRoute: js.UndefOr[js.Array[JsPathSegment]] = js.undefined
  ): JsHttpMountDetails = {
    val obj = js.Dynamic.literal(
      "agentIdBinding"      -> agentIdBinding,
      "createRoute"         -> createRoute,
      "pathPrefix"          -> pathPrefix,
      "phantomAgent"        -> phantomAgent,
      "corsOptions"         -> corsOptions,
      "webhookSuffix"       -> webhookSuffix,
      "staticBindings"      -> staticBindings,
      "filesystemBindings"  -> filesystemBindings,
      "fileResponseHeaders" -> fileResponseHeaders
    )
    authDetails.foreach(a => obj.updateDynamic("authDetails")(a))
    openapiProviderMethod.foreach(p => obj.updateDynamic("openapiProviderMethod")(p))
    obj.asInstanceOf[JsHttpMountDetails]
  }
}

@js.native
sealed trait JsHttpEndpointDetails extends js.Object {
  def httpMethod: JsHttpMethod                                = js.native
  def pathSuffix: js.Array[JsPathSegment]                     = js.native
  def headerVars: js.Array[JsHeaderVariable]                  = js.native
  def queryVars: js.Array[JsQueryVariable]                    = js.native
  def authDetails: js.UndefOr[JsAuthDetails]                  = js.native
  def corsOptions: JsCorsOptions                              = js.native
  def durableStreams: js.UndefOr[JsDurableStreamRouteOptions] = js.native
  def mode: js.UndefOr[String]                                = js.native
}

@js.native
sealed trait JsDurableStreamSlotSource extends js.Object {
  def tag: String                  = js.native
  @JSName("val") def value: String = js.native
}
object JsDurableStreamSlotSource {
  def input(slot: String): JsDurableStreamSlotSource  = JsShape.tagged("input", slot.asInstanceOf[js.Any])
  def output(slot: String): JsDurableStreamSlotSource = JsShape.tagged("output", slot.asInstanceOf[js.Any])
}
@js.native
sealed trait JsDurableStreamSlotOptions extends js.Object {
  def source: JsDurableStreamSlotSource = js.native
  def name: js.UndefOr[String]          = js.native
  def contentType: js.UndefOr[String]   = js.native
}
object JsDurableStreamSlotOptions {
  def apply(
    source: JsDurableStreamSlotSource,
    name: js.UndefOr[String],
    contentType: js.UndefOr[String]
  ): JsDurableStreamSlotOptions = {
    val obj = js.Dynamic.literal("source" -> source)
    name.foreach(v => obj.updateDynamic("name")(v)); contentType.foreach(v => obj.updateDynamic("contentType")(v))
    obj.asInstanceOf[JsDurableStreamSlotOptions]
  }
}
@js.native
sealed trait JsDurableStreamRouteLoadOptions extends js.Object {
  def maxConcurrentReadersPerStream: js.UndefOr[Int]       = js.native
  def maxAppendRequestsPerSecondPerStream: js.UndefOr[Int] = js.native
}
object JsDurableStreamRouteLoadOptions {
  def apply(readers: js.UndefOr[Int], appends: js.UndefOr[Int]): JsDurableStreamRouteLoadOptions = {
    val obj = js.Dynamic.literal(); readers.foreach(v => obj.updateDynamic("maxConcurrentReadersPerStream")(v))
    appends.foreach(v => obj.updateDynamic("maxAppendRequestsPerSecondPerStream")(v));
    obj.asInstanceOf[JsDurableStreamRouteLoadOptions]
  }
}
@js.native
sealed trait JsDurableStreamRouteOptions extends js.Object {
  def slots: js.Array[JsDurableStreamSlotOptions]       = js.native
  def load: js.UndefOr[JsDurableStreamRouteLoadOptions] = js.native
}
object JsDurableStreamRouteOptions {
  def apply(
    slots: js.Array[JsDurableStreamSlotOptions],
    load: js.UndefOr[JsDurableStreamRouteLoadOptions]
  ): JsDurableStreamRouteOptions = {
    val obj = js.Dynamic.literal("slots" -> slots)
    load.foreach(v => obj.updateDynamic("load")(v))
    obj.asInstanceOf[JsDurableStreamRouteOptions]
  }
}

object JsHttpEndpointDetails {
  def apply(
    httpMethod: JsHttpMethod,
    pathSuffix: js.Array[JsPathSegment],
    headerVars: js.Array[JsHeaderVariable],
    queryVars: js.Array[JsQueryVariable],
    corsOptions: JsCorsOptions,
    authDetails: js.UndefOr[JsAuthDetails] = js.undefined,
    durableStreams: js.UndefOr[JsDurableStreamRouteOptions] = js.undefined,
    mode: js.UndefOr[String] = js.undefined
  ): JsHttpEndpointDetails = {
    val obj = js.Dynamic.literal(
      "httpMethod"  -> httpMethod,
      "pathSuffix"  -> pathSuffix,
      "headerVars"  -> headerVars,
      "queryVars"   -> queryVars,
      "corsOptions" -> corsOptions
    )
    authDetails.foreach(a => obj.updateDynamic("authDetails")(a))
    durableStreams.foreach(a => obj.updateDynamic("durableStreams")(a))
    mode.foreach(a => obj.updateDynamic("mode")(a))
    obj.asInstanceOf[JsHttpEndpointDetails]
  }
}

// ---------------------------------------------------------------------------
// CachePolicy, ReadOnlyConfig
// ---------------------------------------------------------------------------

@js.native
sealed trait JsCachePolicy extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsCachePolicyTtl extends JsCachePolicy {
  @JSName("val") def value: js.BigInt = js.native // Duration = u64 nanoseconds
}

object JsCachePolicy {
  def noCache: JsCachePolicy =
    JsShape.tagOnly[JsCachePolicy]("no-cache")

  def untilWrite: JsCachePolicy =
    JsShape.tagOnly[JsCachePolicy]("until-write")

  def ttl(durationNanos: js.BigInt): JsCachePolicy =
    JsShape.tagged[JsCachePolicy]("ttl", durationNanos)
}

@js.native
sealed trait JsReadOnlyConfig extends js.Object {
  def cachePolicy: JsCachePolicy = js.native
}

object JsReadOnlyConfig {
  def apply(cachePolicy: JsCachePolicy): JsReadOnlyConfig =
    js.Dynamic
      .literal("cachePolicy" -> cachePolicy)
      .asInstanceOf[JsReadOnlyConfig]
}

// ---------------------------------------------------------------------------
// Snapshotting
// ---------------------------------------------------------------------------

@js.native
sealed trait JsSnapshottingConfig extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsSnapshottingConfigPeriodic extends JsSnapshottingConfig {
  @JSName("val") def value: js.BigInt = js.native // Duration = u64 nanoseconds
}

@js.native
sealed trait JsSnapshottingConfigEveryN extends JsSnapshottingConfig {
  @JSName("val") def value: Int = js.native
}

object JsSnapshottingConfig {
  def default: JsSnapshottingConfig =
    JsShape.tagOnly[JsSnapshottingConfig]("default")

  def periodic(durationNanos: js.BigInt): JsSnapshottingConfig =
    JsShape.tagged[JsSnapshottingConfig]("periodic", durationNanos)

  def everyNInvocation(n: Int): JsSnapshottingConfig =
    JsShape.tagged[JsSnapshottingConfig]("every-n-invocation", n.asInstanceOf[js.Any])
}

@js.native
sealed trait JsSnapshotting extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsSnapshottingEnabled extends JsSnapshotting {
  @JSName("val") def value: JsSnapshottingConfig = js.native
}

object JsSnapshotting {
  def disabled: JsSnapshotting =
    JsShape.tagOnly[JsSnapshotting]("disabled")

  def enabled(config: JsSnapshottingConfig): JsSnapshotting =
    JsShape.tagged[JsSnapshotting]("enabled", config)
}
