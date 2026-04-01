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

import golem.runtime.AgentMetadata

/**
 * Validation rules for HTTP mount and endpoint configurations.
 *
 * These follow the same semantics as the TS and Rust Golem SDKs:
 *   - Mount variables bind to constructor parameters
 *   - Endpoint variables bind to method parameters
 *   - Various type safety checks
 */
object HttpValidation {

  /**
   * Validates an HTTP endpoint at the trait level (method params are known).
   */
  def validateEndpointVars(
    agentName: String,
    methodName: String,
    endpoint: HttpEndpointDetails,
    methodParamNames: Set[String],
    hasMount: Boolean
  ): Either[String, Unit] =
    validateEndpointVars(agentName, methodName, endpoint, methodParamNames, Set.empty, hasMount)

  /**
   * Validates an HTTP endpoint at the trait level (method params are known),
   * including rejection of Principal-typed parameters from HTTP bindings.
   */
  def validateEndpointVars(
    agentName: String,
    methodName: String,
    endpoint: HttpEndpointDetails,
    methodParamNames: Set[String],
    principalParamNames: Set[String],
    hasMount: Boolean
  ): Either[String, Unit] =
    if (!hasMount)
      Left(
        s"Agent method '$methodName' of '$agentName' defines HTTP endpoints " +
          s"but the agent is not mounted over HTTP. Please specify mount in @agentDefinition."
      )
    else
      for {
        _ <- validateEndpointVarsAreNotPrincipal(methodName, endpoint, principalParamNames)
        _ <- validatePathVars(methodName, endpoint.pathSuffix, methodParamNames)
        _ <- validateHeaderVars(methodName, endpoint.headerVars, methodParamNames)
        _ <- validateQueryVars(methodName, endpoint.queryVars, methodParamNames)
      } yield ()

  private def validateEndpointVarsAreNotPrincipal(
    methodName: String,
    endpoint: HttpEndpointDetails,
    principalParamNames: Set[String]
  ): Either[String, Unit] =
    if (principalParamNames.isEmpty) Right(())
    else {
      val pathPrincipal = endpoint.pathSuffix.collectFirst {
        case PathSegment.PathVariable(v) if principalParamNames.contains(v)          => v
        case PathSegment.RemainingPathVariable(v) if principalParamNames.contains(v) => v
      }
      val headerPrincipal = endpoint.headerVars.collectFirst {
        case hv if principalParamNames.contains(hv.variableName) => hv.variableName
      }
      val queryPrincipal = endpoint.queryVars.collectFirst {
        case qv if principalParamNames.contains(qv.variableName) => qv.variableName
      }
      pathPrincipal.orElse(headerPrincipal).orElse(queryPrincipal) match {
        case Some(varName) =>
          Left(
            s"HTTP endpoint variable '$varName' in method '$methodName' cannot reference a Principal-typed parameter."
          )
        case None => Right(())
      }
    }

  private def validatePathVars(
    methodName: String,
    segments: List[PathSegment],
    methodParamNames: Set[String]
  ): Either[String, Unit] = {
    val missing = segments.collect {
      case PathSegment.PathVariable(v) if !methodParamNames.contains(v)          => v
      case PathSegment.RemainingPathVariable(v) if !methodParamNames.contains(v) => v
    }
    missing.headOption match {
      case Some(varName) =>
        Left(s"HTTP endpoint path variable '$varName' in method '$methodName' is not defined in method parameters.")
      case None => Right(())
    }
  }

  private def validateHeaderVars(
    methodName: String,
    headerVars: List[HeaderVariable],
    methodParamNames: Set[String]
  ): Either[String, Unit] =
    headerVars.find(hv => !methodParamNames.contains(hv.variableName)) match {
      case Some(hv) =>
        Left(
          s"HTTP endpoint header variable '${hv.variableName}' in method '$methodName' is not defined in method parameters."
        )
      case None => Right(())
    }

  private def validateQueryVars(
    methodName: String,
    queryVars: List[QueryVariable],
    methodParamNames: Set[String]
  ): Either[String, Unit] =
    queryVars.find(qv => !methodParamNames.contains(qv.variableName)) match {
      case Some(qv) =>
        Left(
          s"HTTP endpoint query variable '${qv.variableName}' in method '$methodName' is not defined in method parameters."
        )
      case None => Right(())
    }

  /**
   * Validates that the mount path has no catch-all (remaining path) variables.
   */
  def validateNoCatchAllInMount(
    agentName: String,
    mount: HttpMountDetails
  ): Either[String, Unit] =
    mount.pathPrefix.collectFirst { case PathSegment.RemainingPathVariable(name) =>
      name
    } match {
      case Some(name) =>
        Left(s"HTTP mount for agent '$agentName' cannot contain catch-all path variable '{*$name}'")
      case None =>
        Right(())
    }

  /** Validates mount path variables exist in constructor param names. */
  def validateMountVarsExistInConstructor(
    mount: HttpMountDetails,
    constructorParamNames: Set[String]
  ): Either[String, Unit] = {
    val missing = mount.pathPrefix.zipWithIndex.collect {
      case (PathSegment.PathVariable(varName), idx) if !constructorParamNames.contains(varName) =>
        (varName, idx)
    }
    missing.headOption match {
      case Some((varName, idx)) =>
        Left(
          s"HTTP mount path variable '$varName' (in path segment $idx) is not defined in the agent constructor."
        )
      case None => Right(())
    }
  }

  /** Validates all constructor params are satisfied by mount path variables. */
  def validateConstructorVarsSatisfied(
    mount: HttpMountDetails,
    constructorParamNames: Set[String]
  ): Either[String, Unit] = {
    val providedVars = mount.pathPrefix.collect { case PathSegment.PathVariable(name) =>
      name
    }.toSet

    constructorParamNames.find(param => !providedVars.contains(param)) match {
      case Some(param) =>
        Left(s"Agent constructor variable '$param' is not provided by the HTTP mount path.")
      case None =>
        Right(())
    }
  }

  /**
   * Validates that mount path variables do not reference Principal-typed
   * constructor parameters.
   */
  def validateMountVarsAreNotPrincipal(
    agentName: String,
    mount: HttpMountDetails,
    principalParamNames: Set[String]
  ): Either[String, Unit] =
    if (principalParamNames.isEmpty) Right(())
    else
      mount.pathPrefix.collectFirst {
        case PathSegment.PathVariable(v) if principalParamNames.contains(v) => v
      } match {
        case Some(varName) =>
          Left(
            s"HTTP mount path variable '$varName' for agent '$agentName' cannot reference a Principal-typed constructor parameter."
          )
        case None => Right(())
      }

  /**
   * Runs all mount-level validations (called from implementation-level macro).
   */
  def validateHttpMount(
    agentName: String,
    mount: HttpMountDetails,
    constructorParamNames: Set[String]
  ): Either[String, Unit] =
    validateHttpMount(agentName, mount, constructorParamNames, Set.empty)

  /**
   * Runs all mount-level validations including Principal rejection.
   */
  def validateHttpMount(
    agentName: String,
    mount: HttpMountDetails,
    constructorParamNames: Set[String],
    principalParamNames: Set[String]
  ): Either[String, Unit] =
    for {
      _ <- validateNoCatchAllInMount(agentName, mount)
      _ <- validateMountVarsAreNotPrincipal(agentName, mount, principalParamNames)
      _ <- validateMountVarsExistInConstructor(mount, constructorParamNames)
      _ <- validateConstructorVarsSatisfied(mount, constructorParamNames)
    } yield ()

  /**
   * Validates the HTTP mount against constructor parameter names extracted from
   * the agent's constructor schema. Called from generated code in the
   * `@agentImplementation` macro.
   *
   * @throws IllegalArgumentException
   *   if validation fails
   */
  def validateHttpMountFromMetadata(metadata: AgentMetadata): Unit =
    metadata.httpMount.foreach { mount =>
      val constructorParamNames = extractConstructorParamNames(metadata.constructor)
      validateHttpMount(metadata.name, mount, constructorParamNames) match {
        case Left(err) => throw new IllegalArgumentException(err)
        case Right(()) => ()
      }
    }

  /**
   * Extracts constructor parameter names from the agent's constructor schema.
   *
   * The parameter names depend on how the agent's `class Id` is defined:
   *
   *   - '''Single parameter''' (e.g. `class Id(val value: String)`): produces
   *     one parameter named `"value"`. The mount path must use `{value}` to
   *     refer to it.
   *   - '''Multiple parameters''' (e.g.
   *     `class Id(val arg0: String, val arg1: Int)`): produces parameters named
   *     `"arg0"`, `"arg1"`, etc. The mount path must use `{arg0}`, `{arg1}`,
   *     etc.
   *   - '''No id''': produces no parameters. Mount paths must not contain
   *     variables.
   */
  private def extractConstructorParamNames(schema: golem.data.StructuredSchema): Set[String] =
    schema match {
      case golem.data.StructuredSchema.Tuple(elements) =>
        elements.map(_.name).toSet
      case _ => Set.empty
    }
}
