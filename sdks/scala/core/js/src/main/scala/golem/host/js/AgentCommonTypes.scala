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

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// golem:agent/common@2.0.0  –  JS facade traits
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Principal  –  tagged union
// ---------------------------------------------------------------------------

@js.native
sealed trait JsOidcPrincipal extends js.Object {
  def sub: String                           = js.native
  def issuer: String                        = js.native
  def email: js.UndefOr[String]             = js.native
  def name: js.UndefOr[String]              = js.native
  def emailVerified: js.UndefOr[Boolean]    = js.native
  def givenName: js.UndefOr[String]         = js.native
  def familyName: js.UndefOr[String]        = js.native
  def picture: js.UndefOr[String]           = js.native
  def preferredUsername: js.UndefOr[String] = js.native
  def claims: String                        = js.native
}

object JsOidcPrincipal {
  def apply(
    sub: String,
    issuer: String,
    claims: String,
    email: js.UndefOr[String] = js.undefined,
    name: js.UndefOr[String] = js.undefined,
    emailVerified: js.UndefOr[Boolean] = js.undefined,
    givenName: js.UndefOr[String] = js.undefined,
    familyName: js.UndefOr[String] = js.undefined,
    picture: js.UndefOr[String] = js.undefined,
    preferredUsername: js.UndefOr[String] = js.undefined
  ): JsOidcPrincipal = {
    val obj = js.Dynamic.literal("sub" -> sub, "issuer" -> issuer, "claims" -> claims)
    email.foreach(v => obj.updateDynamic("email")(v))
    name.foreach(v => obj.updateDynamic("name")(v))
    emailVerified.foreach(v => obj.updateDynamic("emailVerified")(v))
    givenName.foreach(v => obj.updateDynamic("givenName")(v))
    familyName.foreach(v => obj.updateDynamic("familyName")(v))
    picture.foreach(v => obj.updateDynamic("picture")(v))
    preferredUsername.foreach(v => obj.updateDynamic("preferredUsername")(v))
    obj.asInstanceOf[JsOidcPrincipal]
  }
}

@js.native
sealed trait JsAgentPrincipal extends js.Object {
  def agentId: JsAgentId = js.native
}

object JsAgentPrincipal {
  def apply(agentId: JsAgentId): JsAgentPrincipal =
    js.Dynamic.literal("agentId" -> agentId).asInstanceOf[JsAgentPrincipal]
}

@js.native
sealed trait JsGolemUserPrincipal extends js.Object {
  def accountId: JsAccountId = js.native
}

object JsGolemUserPrincipal {
  def apply(accountId: JsAccountId): JsGolemUserPrincipal =
    js.Dynamic.literal("accountId" -> accountId).asInstanceOf[JsGolemUserPrincipal]
}

@js.native
sealed trait JsPrincipal extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsPrincipalOidc extends JsPrincipal {
  @JSName("val") def value: JsOidcPrincipal = js.native
}

@js.native
sealed trait JsPrincipalAgent extends JsPrincipal {
  @JSName("val") def value: JsAgentPrincipal = js.native
}

@js.native
sealed trait JsPrincipalGolemUser extends JsPrincipal {
  @JSName("val") def value: JsGolemUserPrincipal = js.native
}

object JsPrincipal {
  def oidc(value: JsOidcPrincipal): JsPrincipal =
    JsShape.tagged[JsPrincipal]("oidc", value)

  def agent(value: JsAgentPrincipal): JsPrincipal =
    JsShape.tagged[JsPrincipal]("agent", value)

  def golemUser(value: JsGolemUserPrincipal): JsPrincipal =
    JsShape.tagged[JsPrincipal]("golem-user", value)

  def anonymous: JsPrincipal =
    JsShape.tagOnly[JsPrincipal]("anonymous")
}

// ---------------------------------------------------------------------------
// HTTP types
// ---------------------------------------------------------------------------

@js.native
sealed trait JsHttpMethod extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsHttpMethodCustom extends JsHttpMethod {
  @JSName("val") def value: String = js.native
}

object JsHttpMethod {
  def get: JsHttpMethod     = JsShape.tagOnly[JsHttpMethod]("get")
  def head: JsHttpMethod    = JsShape.tagOnly[JsHttpMethod]("head")
  def post: JsHttpMethod    = JsShape.tagOnly[JsHttpMethod]("post")
  def put: JsHttpMethod     = JsShape.tagOnly[JsHttpMethod]("put")
  def delete: JsHttpMethod  = JsShape.tagOnly[JsHttpMethod]("delete")
  def connect: JsHttpMethod = JsShape.tagOnly[JsHttpMethod]("connect")
  def options: JsHttpMethod = JsShape.tagOnly[JsHttpMethod]("options")
  def trace: JsHttpMethod   = JsShape.tagOnly[JsHttpMethod]("trace")
  def patch: JsHttpMethod   = JsShape.tagOnly[JsHttpMethod]("patch")

  def custom(method: String): JsHttpMethod =
    JsShape.tagged[JsHttpMethod]("custom", method.asInstanceOf[js.Any])
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
sealed trait JsPathSegmentLiteral extends JsPathSegment {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsPathSegmentSystemVariable extends JsPathSegment {
  @JSName("val") def value: JsSystemVariable = js.native
}

@js.native
sealed trait JsPathSegmentPathVariable extends JsPathSegment {
  @JSName("val") def value: JsPathVariable = js.native
}

@js.native
sealed trait JsPathSegmentRemainingPathVariable extends JsPathSegment {
  @JSName("val") def value: JsPathVariable = js.native
}

object JsPathSegment {
  def literal(value: String): JsPathSegment =
    JsShape.tagged[JsPathSegment]("literal", value.asInstanceOf[js.Any])

  def systemVariable(value: JsSystemVariable): JsPathSegment =
    JsShape.tagged[JsPathSegment]("system-variable", value.asInstanceOf[js.Any])

  def pathVariable(value: JsPathVariable): JsPathSegment =
    JsShape.tagged[JsPathSegment]("path-variable", value)

  def remainingPathVariable(value: JsPathVariable): JsPathSegment =
    JsShape.tagged[JsPathSegment]("remaining-path-variable", value)
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
sealed trait JsHttpMountDetails extends js.Object {
  def pathPrefix: js.Array[JsPathSegment]    = js.native
  def authDetails: js.UndefOr[JsAuthDetails] = js.native
  def phantomAgent: Boolean                  = js.native
  def corsOptions: JsCorsOptions             = js.native
  def webhookSuffix: js.Array[JsPathSegment] = js.native
}

object JsHttpMountDetails {
  def apply(
    pathPrefix: js.Array[JsPathSegment],
    phantomAgent: Boolean,
    corsOptions: JsCorsOptions,
    webhookSuffix: js.Array[JsPathSegment],
    authDetails: js.UndefOr[JsAuthDetails] = js.undefined
  ): JsHttpMountDetails = {
    val obj = js.Dynamic.literal(
      "pathPrefix"    -> pathPrefix,
      "phantomAgent"  -> phantomAgent,
      "corsOptions"   -> corsOptions,
      "webhookSuffix" -> webhookSuffix
    )
    authDetails.foreach(a => obj.updateDynamic("authDetails")(a))
    obj.asInstanceOf[JsHttpMountDetails]
  }
}

@js.native
sealed trait JsHttpEndpointDetails extends js.Object {
  def httpMethod: JsHttpMethod               = js.native
  def pathSuffix: js.Array[JsPathSegment]    = js.native
  def headerVars: js.Array[JsHeaderVariable] = js.native
  def queryVars: js.Array[JsQueryVariable]   = js.native
  def authDetails: js.UndefOr[JsAuthDetails] = js.native
  def corsOptions: JsCorsOptions             = js.native
}

object JsHttpEndpointDetails {
  def apply(
    httpMethod: JsHttpMethod,
    pathSuffix: js.Array[JsPathSegment],
    headerVars: js.Array[JsHeaderVariable],
    queryVars: js.Array[JsQueryVariable],
    corsOptions: JsCorsOptions,
    authDetails: js.UndefOr[JsAuthDetails] = js.undefined
  ): JsHttpEndpointDetails = {
    val obj = js.Dynamic.literal(
      "httpMethod"  -> httpMethod,
      "pathSuffix"  -> pathSuffix,
      "headerVars"  -> headerVars,
      "queryVars"   -> queryVars,
      "corsOptions" -> corsOptions
    )
    authDetails.foreach(a => obj.updateDynamic("authDetails")(a))
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
  def usesPrincipal: Boolean     = js.native
}

object JsReadOnlyConfig {
  def apply(cachePolicy: JsCachePolicy, usesPrincipal: Boolean): JsReadOnlyConfig =
    js.Dynamic
      .literal("cachePolicy" -> cachePolicy, "usesPrincipal" -> usesPrincipal)
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
