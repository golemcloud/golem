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

package golem.codegen.rpc

import golem.codegen.discovery.SourceDiscovery
import golem.codegen.ir.AgentSurfaceIR._

import scala.meta._
import scala.meta.parsers._

/**
 * Code generator for Golem RPC client objects.
 *
 * Given agent surface metadata, generates client objects with:
 *   - `XRemote` trait with per-method wrapper classes for natural call syntax
 *   - Mode-aware constructors: durable agents get `get`/`getWithConfig` plus
 *     `getPhantom`/`newPhantom`/`getPhantomWithConfig`/`newPhantomWithConfig`;
 *     ephemeral agents get only the phantom variants
 *
 * Call syntax on the returned `XRemote`:
 *   - `remote.process(1, "hello")` — await the result via `apply`
 *   - `remote.process.trigger(1, "hello")` — fire-and-forget
 *   - `remote.process.scheduleAt(1, "hello", when)` — schedule at a time
 *   - `remote.process.withScopeCard(card)(1, "hello")` — await with a scope
 *     card; the scoped view is the nested
 *     `ProcessRemoteMethod#GolemScopeCardCall` class
 *   - `remote.withDefaultScopeCard(card)` — a client whose awaited calls carry
 *     `card`
 *
 * Generated clients extend `golem.AgentCompanionBase[Trait]` so the existing
 * macro infrastructure can locate the trait type parameter.
 */
object RpcCodegen {

  final case class GeneratedFile(relativePath: String, content: String)

  final case class Warning(message: String)

  final case class Error(message: String)

  final case class Result(
    files: Seq[GeneratedFile],
    warnings: Seq[Warning],
    errors: Seq[Error] = Nil
  )

  /**
   * Members of the generated remote trait; an agent method cannot use these
   * names.
   */
  val ReservedRemoteMemberNames: Set[String] = Set("withDefaultScopeCard", "withoutDefaultScopeCard")

  /**
   * The nested scoped-call class of every generated method class. A bare type
   * of this name in a method signature would resolve to the nested class, so
   * signatures cannot use it.
   */
  val ScopedCallClassName: String = "GolemScopeCardCall"

  private val ScopedCallClassReference = ("(?<![\\w.$])" + ScopedCallClassName + "(?![\\w$])").r

  /**
   * Backticks quote an identifier without changing it, so they are removed
   * first.
   */
  private def usesScopedCallClassName(method: MethodSurface): Boolean =
    (method.returnTypeExpr :: remoteParams(method).map(_.typeExpr))
      .exists(typeExpr => ScopedCallClassReference.findFirstIn(typeExpr.replace("`", "")).isDefined)

  def generate(
    agents: List[AgentSurface],
    existingObjects: Seq[SourceDiscovery.ExistingObject]
  ): Result = {
    val warnings = List.newBuilder[Warning]
    val files    = List.newBuilder[GeneratedFile]
    val errors   = List.newBuilder[Error]

    val existingByFqn: Set[String] = existingObjects.map { obj =>
      if (obj.pkg.isEmpty) obj.name else s"${obj.pkg}.${obj.name}"
    }.toSet

    agents.filter(_.metadata.kind == "regular").foreach { agent =>
      val clientName = s"${agent.simpleName}Client"
      val clientFqn  =
        if (agent.packageName.isEmpty) clientName
        else s"${agent.packageName}.$clientName"

      val generated = generatedMethods(agent)
      val reserved  = generated.map(_.name).filter(ReservedRemoteMemberNames.contains)
      val shadowed  = generated.filter(usesScopedCallClassName).map(_.name)
      if (existingByFqn.contains(clientFqn)) {
        warnings += Warning(
          s"Skipping RPC client generation for ${agent.traitFqn}: " +
            s"object $clientFqn already exists. Remove the handwritten client to enable codegen."
        )
      } else if (agent.sourceScopes.nonEmpty && scopesPackageName(agent) != agent.packageName) {
        errors += Error(
          s"Agent ${agent.traitFqn} has source package clauses ${scopesPackageName(agent)}, " +
            s"which do not form its package ${agent.packageName}."
        )
      } else if (reserved.nonEmpty) {
        errors += Error(
          s"Agent ${agent.traitFqn} declares method(s) ${reserved.mkString(", ")}, which the generated RPC client " +
            "reserves for its scope-card API. Rename the method(s)."
        )
      } else if (shadowed.nonEmpty) {
        errors += Error(
          s"Agent ${agent.traitFqn} method(s) ${shadowed.mkString(", ")} use the bare type name " +
            s"$ScopedCallClassName, which the generated RPC client reserves for its scoped-call class. " +
            "Rename the type or refer to it by its qualified name."
        )
      } else {
        val content     = generateCompanion(agent)
        val packagePath =
          if (agent.packageName.isEmpty) ""
          else agent.packageName.replace('.', '/') + "/"
        val path = s"$packagePath${clientName}.scala"
        files += GeneratedFile(relativePath = path, content = content)
      }
    }

    Result(files = files.result(), warnings = warnings.result(), errors = errors.result())
  }

  /** Methods that get a generated wrapper: those with valid principalParams. */
  private def generatedMethods(agent: AgentSurface): List[MethodSurface] =
    agent.methods.filter(m => m.principalParams.length == m.params.length)

  /** Filter out Principal parameters from a method surface. */
  private def remoteParams(method: MethodSurface): List[ParamSurface] =
    method.params.zip(method.principalParams).collect { case (p, false) =>
      p
    }

  /** Capitalize the first character of a string. */
  private def capitalize(s: String): String =
    if (s.isEmpty) s else s.head.toUpper.toString + s.tail

  /**
   * Unwrap `Future[T]` to get `T`. Returns the original type if not a Future
   * wrapper.
   */
  private[rpc] def unwrapFutureType(returnTypeExpr: String): String = {
    val tpe =
      try {
        dialects.Scala3(returnTypeExpr).parse[Type].get
      } catch {
        case _: Exception => return returnTypeExpr
      }

    tpe match {
      case Type.Apply(name, args) if args.size == 1 =>
        val nameStr = name.syntax
        if (
          nameStr == "Future" ||
          nameStr == "scala.concurrent.Future" ||
          nameStr == "_root_.scala.concurrent.Future"
        ) {
          args.head.syntax
        } else {
          returnTypeExpr
        }
      case _ => returnTypeExpr
    }
  }

  /**
   * The package clauses of the generated file, each followed by the imports the
   * agent trait sees at that level, so the method signatures copied from the
   * trait resolve as they do in the trait's file. A package clause that follows
   * an import is braced, as Scala requires.
   */
  private def wrapInSourceScopes(agent: AgentSurface, body: String): String =
    if (agent.sourceScopes.isEmpty) {
      if (agent.packageName.isEmpty) body else s"package ${agent.packageName}\n\n$body"
    } else {
      val sb          = new StringBuilder
      var importsSeen = false
      var openBraces  = 0
      agent.sourceScopes.foreach { scope =>
        if (scope.packageRef.nonEmpty) {
          if (importsSeen) {
            sb.append(s"package ${scope.packageRef} {\n\n")
            openBraces += 1
          } else sb.append(s"package ${scope.packageRef}\n\n")
        }
        if (scope.imports.nonEmpty) {
          scope.imports.foreach(i => sb.append(i).append('\n'))
          sb.append('\n')
          importsSeen = true
        }
      }
      sb.append(body)
      (0 until openBraces).foreach(_ => sb.append("}\n"))
      sb.toString
    }

  private def scopesPackageName(agent: AgentSurface): String =
    agent.sourceScopes.map(_.packageRef).filter(_.nonEmpty).mkString(".")

  /**
   * The agent trait, root-qualified so that an import carried from the trait's
   * file cannot shadow it. A trait in the empty package has no qualified name.
   */
  private def traitReference(agent: AgentSurface): String =
    if (agent.packageName.isEmpty) agent.simpleName else s"_root_.${agent.packageName}.${agent.simpleName}"

  private def generateCompanion(agent: AgentSurface): String = {
    val simpleName = agent.simpleName
    val traitRef   = traitReference(agent)
    val typeName   = agent.typeName

    val constructorType = constructorTypeExpr(agent.constructor)

    val remoteMethods = generatedMethods(agent)

    val sb = new StringBuilder

    sb.append("/** Generated by Golem RPC codegen. Do not edit. */\n")
    sb.append(s"object ${simpleName}Client extends _root_.golem.AgentCompanionBase[$traitRef] {\n\n")

    // type Id (private — implementation detail)
    sb.append(s"  private type Id = $constructorType\n\n")

    // typeName
    sb.append(s"""  val typeName: _root_.scala.Predef.String = "$typeName"\n\n""")

    // Structurally generated client codecs and descriptor. The reflective
    // AgentType API remains available explicitly through AgentClient.
    sb.append(s"  private lazy val agentType: _root_.golem.runtime.WireAgentClientType[$traitRef, Id] =\n")
    sb.append(s"    _root_.golem.runtime.macros.AgentClientMacro.wireType[$traitRef]\n")
    sb.append(s"      .asInstanceOf[_root_.golem.runtime.WireAgentClientType[$traitRef, Id]]\n\n")

    // Generate XRemote trait, per-method classes, and mode-aware constructors
    if (remoteMethods.nonEmpty) {
      generateRemoteTrait(sb, agent, remoteMethods)
    }

    sb.append("}\n")

    wrapInSourceScopes(agent, sb.toString)
  }

  private def generateRemoteTrait(
    sb: StringBuilder,
    agent: AgentSurface,
    remoteMethods: List[MethodSurface]
  ): Unit = {
    val simpleName = agent.simpleName
    val traitRef   = traitReference(agent)
    val remoteName = s"${simpleName}Remote"

    // Per-method wrapper classes
    remoteMethods.foreach { method =>
      generateRemoteMethodClass(sb, simpleName, traitRef, method, agent.metadata.mode == "ephemeral")
    }

    // XRemote trait
    sb.append(s"  trait $remoteName {\n")
    remoteMethods.foreach { method =>
      val className = s"${capitalize(method.name)}RemoteMethod"
      sb.append(s"    val ${method.name}: $className\n")
    }
    sb.append("\n")
    sb.append("    /** A client whose awaited calls borrow `card` as the scope card unless a call sets its own. */\n")
    sb.append(s"    def withDefaultScopeCard(card: $ScopeCardType): $remoteName\n\n")
    sb.append("    /** A client whose awaited calls carry no default scope card. */\n")
    sb.append(s"    def withoutDefaultScopeCard: $remoteName\n")
    sb.append(s"  }\n\n")

    // Private bindRemote
    sb.append(s"  private def bindRemote(\n")
    sb.append(s"    resolved: _root_.golem.runtime.rpc.AgentClientRuntime.WireResolvedAgent[$traitRef]\n")
    sb.append(s"  ): $remoteName =\n")
    sb.append(s"    new $remoteName {\n")
    remoteMethods.foreach { method =>
      val className = s"${capitalize(method.name)}RemoteMethod"
      sb.append(s"""      val ${method.name} = new $className(resolved)\n""")
    }
    sb.append(s"      def withDefaultScopeCard(card: $ScopeCardType): $remoteName =\n")
    sb.append("        bindRemote(resolved.withScopeCard(card))\n")
    sb.append(s"      def withoutDefaultScopeCard: $remoteName =\n")
    sb.append("        bindRemote(resolved.withoutScopeCard)\n")
    sb.append(s"    }\n\n")

    // Mode-aware constructors
    generateModeAwareConstructors(sb, agent, remoteName)
  }

  private def generateRemoteMethodClass(
    sb: StringBuilder,
    simpleName: String,
    traitRef: String,
    method: MethodSurface,
    ephemeral: Boolean
  ): Unit = {
    val className  = s"${capitalize(method.name)}RemoteMethod"
    val rParams    = remoteParams(method)
    val resultType = unwrapFutureType(method.returnTypeExpr)

    // Input type for the packed method call
    val packedInputType = rParams match {
      case Nil      => "_root_.scala.Unit"
      case p :: Nil => p.typeExpr
      case _        => "_root_.scala.collection.immutable.Vector[_root_.scala.Any]"
    }

    val outputType = resultType

    sb.append(s"  final class $className private[$simpleName" + "Client] (\n")
    sb.append(s"    resolved: _root_.golem.runtime.rpc.AgentClientRuntime.WireResolvedAgent[$traitRef]\n")
    val methodNameLit = "\"" + method.name + "\""
    sb.append(
      s"  ) extends _root_.golem.runtime.rpc.AbstractWireRemoteMethod[$traitRef, $packedInputType, $outputType](resolved, $methodNameLit) {\n"
    )

    val packExpr = rParams match {
      case Nil      => "()"
      case p :: Nil => p.name
      case ps       => "_root_.scala.Vector[_root_.scala.Any](" + ps.map(_.name).mkString(", ") + ")"
    }

    val paramDecls      = rParams.map(p => s"${p.name}: ${p.typeExpr}").mkString(", ")
    val awaitResultType = if (ephemeral) s"_root_.golem.runtime.rpc.InvocationResult[$outputType]" else outputType
    sb.append(s"    def apply($paramDecls): _root_.scala.concurrent.Future[$awaitResultType] =\n")
    sb.append(s"      ${if (ephemeral) "awaitWithMetadata" else "awaitWith"}($packExpr)\n\n")

    if (ephemeral) {
      sb.append(
        s"    def cancelable($paramDecls): _root_.scala.concurrent.Future[_root_.golem.runtime.rpc.CancelableAsyncInvocation[$outputType]] =\n"
      )
      sb.append(s"      cancelableAwaitWithMetadata($packExpr)\n\n")
    } else {
      sb.append(
        s"    def cancelable($paramDecls): (_root_.scala.concurrent.Future[$outputType], _root_.golem.runtime.rpc.CancellationToken) =\n"
      )
      sb.append(s"      cancelableAwaitWith($packExpr)\n\n")
    }

    val receiptType = if (ephemeral) "_root_.golem.runtime.rpc.InvocationReceipt" else "_root_.scala.Unit"
    sb.append(s"    def trigger($paramDecls): _root_.scala.concurrent.Future[$receiptType] =\n")
    sb.append(s"      ${if (ephemeral) "triggerWithMetadata" else "triggerWith"}($packExpr)\n\n")

    val scheduleParamDecls =
      if (paramDecls.isEmpty) "when: _root_.golem.Datetime"
      else s"$paramDecls, when: _root_.golem.Datetime"
    sb.append(s"    def scheduleAt($scheduleParamDecls): _root_.scala.concurrent.Future[$receiptType] =\n")
    sb.append(s"      ${if (ephemeral) "scheduleWithMetadata" else "scheduleWith"}($packExpr, when)\n\n")

    val scheduleCancelableParamDecls = scheduleParamDecls
    sb.append(
      s"    def scheduleCancelableAt($scheduleCancelableParamDecls): _root_.scala.concurrent.Future[${
          if (ephemeral) "_root_.golem.runtime.rpc.CancelableInvocationReceipt"
          else "_root_.golem.runtime.rpc.CancellationToken"
        }] =\n"
    )
    sb.append(
      s"      ${if (ephemeral) "scheduleCancelableWithMetadata" else "scheduleCancelableWith"}($packExpr, when)\n\n"
    )

    sb.append("    /** The awaited calls of this method, borrowing `card` as the scope card. */\n")
    sb.append(s"    def withScopeCard(card: $ScopeCardType): $ScopedCallClassName =\n")
    sb.append(s"      new $ScopedCallClassName(resolved.withScopeCard(card))\n\n")

    sb.append(s"    final class $ScopedCallClassName private[$className] (\n")
    sb.append(s"      scoped: _root_.golem.runtime.rpc.AgentClientRuntime.WireResolvedAgent[$traitRef]\n")
    sb.append(
      s"    ) extends _root_.golem.runtime.rpc.AbstractWireRemoteMethod[$traitRef, $packedInputType, $outputType](scoped, $methodNameLit) {\n"
    )
    sb.append(s"      def apply($paramDecls): _root_.scala.concurrent.Future[$awaitResultType] =\n")
    sb.append(s"        ${if (ephemeral) "awaitWithMetadata" else "awaitWith"}($packExpr)\n\n")
    if (ephemeral) {
      sb.append(
        s"      def cancelable($paramDecls): _root_.scala.concurrent.Future[_root_.golem.runtime.rpc.CancelableAsyncInvocation[$outputType]] =\n"
      )
      sb.append(s"        cancelableAwaitWithMetadata($packExpr)\n")
    } else {
      sb.append(
        s"      def cancelable($paramDecls): (_root_.scala.concurrent.Future[$outputType], _root_.golem.runtime.rpc.CancellationToken) =\n"
      )
      sb.append(s"        cancelableAwaitWith($packExpr)\n")
    }
    sb.append("    }\n")
    sb.append(s"  }\n\n")
  }

  private val ScopeCardType = "_root_.golem.host.PermissionsApi.ScopeCard"

  /**
   * Build the packed constructor input expression from unpacked param names.
   */
  private def constructorPackExpr(ctorParams: List[ParamSurface]): String =
    ctorParams match {
      case Nil      => "()"
      case p :: Nil => p.name
      case ps       => "_root_.scala.Vector[_root_.scala.Any](" + ps.map(_.name).mkString(", ") + ")"
    }

  /**
   * Build a camelCase parameter name from a config field path, e.g. List("db",
   * "host") → "dbHost".
   */
  private def configParamName(path: List[String]): String =
    path match {
      case Nil      => ""
      case h :: Nil => h
      case h :: t   => h + t.map(capitalize).mkString
    }

  private def generateModeAwareConstructors(
    sb: StringBuilder,
    agent: AgentSurface,
    remoteName: String
  ): Unit = {
    val traitRef     = traitReference(agent)
    val ctorParams   = agent.constructor.params
    val mode         = agent.metadata.mode
    val configFields = agent.configFields

    val paramDecls = ctorParams.map(p => s"${p.name}: ${p.typeExpr}").mkString(", ")
    val packExpr   = constructorPackExpr(ctorParams)

    def resolveCall(phantom: Option[String], config: Option[String]): String = {
      val ph  = phantom.fold("_root_.scala.None")(p => s"_root_.scala.Some($p)")
      val cfg = config.getOrElse("_root_.scala.Nil")
      s"_root_.golem.runtime.rpc.AgentClientRuntime.resolveWire[$traitRef, Id](agentType, $packExpr, $ph, $cfg)"
    }

    def emitConstructor(
      name: String,
      extraParamsBefore: String,
      extraParamsAfter: String,
      phantom: Option[String],
      config: Option[String]
    ): Unit = {
      val allParams = Seq(extraParamsBefore, paramDecls, extraParamsAfter).filter(_.nonEmpty).mkString(", ")
      sb.append(s"  def $name($allParams): $remoteName =\n")
      sb.append(s"    ${resolveCall(phantom, config)} match {\n")
      sb.append(s"      case _root_.scala.Right(resolved) => bindRemote(resolved)\n")
      sb.append(s"      case _root_.scala.Left(err) => throw _root_.scala.scalajs.js.JavaScriptException(err)\n")
      sb.append(s"    }\n\n")
    }

    /** Generate the config override parameters as `Option[T] = None` */
    def configParamDecls: String =
      configFields.map { cf =>
        val pName = configParamName(cf.path)
        s"$pName: _root_.scala.Option[${cf.typeExpr}] = _root_.scala.None"
      }.mkString(", ")

    /**
     * Generate the body that builds a List[ConfigOverride] from non-None params
     */
    def configOverrideListExpr: String = {
      val builders = configFields.map { cf =>
        val pName   = configParamName(cf.path)
        val pathLit = cf.path.map(p => s""""$p"""").mkString(", ")
        s"$pName.map(v => _root_.golem.config.ConfigOverride[${cf.typeExpr}](_root_.scala.List($pathLit), v))"
      }
      "_root_.scala.List(" + builders.mkString(", ") + ").flatten"
    }

    def emitWithConfigConstructor(
      name: String,
      extraParamsBefore: String,
      extraParamsAfter: String,
      phantom: Option[String]
    ): Unit = {
      val configParams  = configParamDecls
      val allExtra      = Seq(extraParamsAfter, configParams).filter(_.nonEmpty).mkString(", ")
      val allParams     = Seq(extraParamsBefore, paramDecls, allExtra).filter(_.nonEmpty).mkString(", ")
      val overridesExpr = configOverrideListExpr
      val resolveExpr   = resolveCall(phantom, Some(overridesExpr))
      sb.append(s"  def $name($allParams): $remoteName =\n")
      sb.append(s"    $resolveExpr match {\n")
      sb.append(s"      case _root_.scala.Right(resolved) => bindRemote(resolved)\n")
      sb.append(s"      case _root_.scala.Left(err) => throw _root_.scala.scalajs.js.JavaScriptException(err)\n")
      sb.append(s"    }\n\n")
    }

    // Durable agents get: get, getPhantom, newPhantom
    // Ephemeral agents get: getPhantom, newPhantom
    // WithConfig variants only generated when configFields is non-empty
    if (mode == "durable" || mode == "") {
      emitConstructor("get", "", "", phantom = None, config = None)
      if (configFields.nonEmpty) {
        emitWithConfigConstructor("getWithConfig", "", "", phantom = None)
      }
    }

    if (mode == "ephemeral") {
      emitConstructor("newPhantom", "", "", phantom = None, config = None)
      if (configFields.nonEmpty) emitWithConfigConstructor("newPhantomWithConfig", "", "", phantom = None)
      emitConstructor("getPhantom", "", "phantom: _root_.java.util.UUID", phantom = Some("phantom"), config = None)
      if (configFields.nonEmpty)
        emitWithConfigConstructor(
          "getPhantomWithConfig",
          "",
          "phantom: _root_.java.util.UUID",
          phantom = Some("phantom")
        )
      return
    }

    // Durable phantom constructors retain their existing behavior.
    emitConstructor("getPhantom", "", "phantom: _root_.java.util.UUID", phantom = Some("phantom"), config = None)
    val ctorRefs         = if (ctorParams.isEmpty) "" else ctorParams.map(_.name).mkString(", ") + ", "
    val newPhantomParams = if (paramDecls.isEmpty) "" else paramDecls
    sb.append(s"  def newPhantom($newPhantomParams): $remoteName =\n")
    sb.append(s"    getPhantom(${ctorRefs}_root_.golem.HostApi.generateIdempotencyKey())\n\n")
    if (configFields.nonEmpty) {
      emitWithConfigConstructor("getPhantomWithConfig", "", "phantom: _root_.java.util.UUID", phantom = Some("phantom"))
      val newPhantomWithConfigParams = {
        val configParams = configParamDecls
        val extra        =
          Seq("phantom: _root_.java.util.UUID" +: Nil, configParams +: Nil).flatten.filter(_.nonEmpty).mkString(", ")
        Seq(paramDecls, extra).filter(_.nonEmpty).mkString(", ")
      }
      // newPhantomWithConfig delegates, need to forward config params
      val configParamNames = configFields.map(cf => configParamName(cf.path))
      val forwardArgs      = configParamNames.map(n => s"$n = $n").mkString(", ")
      val phantomAndConfig = s"_root_.golem.HostApi.generateIdempotencyKey(), $forwardArgs"
      val delegateArgs     =
        if (ctorParams.isEmpty) phantomAndConfig
        else ctorParams.map(_.name).mkString(", ") + ", " + phantomAndConfig
      val npcParams = {
        val configParams = configParamDecls
        Seq(paramDecls, configParams).filter(_.nonEmpty).mkString(", ")
      }
      sb.append(s"  def newPhantomWithConfig($npcParams): $remoteName =\n")
      sb.append(s"    getPhantomWithConfig($delegateArgs)\n\n")
    }
  }

  private def constructorTypeExpr(ctor: ConstructorSurface): String =
    ctor.params match {
      case Nil      => "_root_.scala.Unit"
      case p :: Nil => p.typeExpr
      case _        => "_root_.scala.collection.immutable.Vector[_root_.scala.Any]"
    }
}
