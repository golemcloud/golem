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

package golem.mill

import mill.*
import mill.api.BuildCtx
import mill.scalalib.*
import mill.scalajslib.config.ScalaJSConfigModule
import org.scalajs.linker.{interface => sjs}

import java.io.ByteArrayOutputStream
import java.security.MessageDigest

import golem.codegen.autoregister.AutoRegisterCodegen
import golem.codegen.discovery.SourceDiscovery
import golem.codegen.pipeline.CodegenPipeline

/**
 * Mill mixin that provides Golem Scala.js agent build wiring.
 *
 * Features (matching sbt GolemPlugin):
 *  - Auto-registration source generation (scans `@agentImplementation` classes)
 *  - `golemPrepare` — writes `agent_guest.wasm` and `scala-js-template.yaml` to `.generated/`
 *  - `golemBuildComponent` — builds the Scala.js bundle for golem-cli to consume
 *  - `moduleInitializers` — auto-configured for the generated `RegisterAgents` entrypoint
 *
 * External usage (example):
 *
 * ```scala
 * import $ivy.`dev.zio::zio-golem-mill:<VERSION>`
 * import golem.mill.GolemAutoRegister
 *
 * object demo extends GolemAutoRegister {
 *   def scalaVersion   = "3.3.7"
 *   def golemBasePackage = Task(Some("demo"))
 * }
 * ```
 */
trait GolemAutoRegister extends ScalaJSConfigModule {

  private final case class GuestArtifact(fileName: String)

  private val AgentGuestArtifact               = GuestArtifact("agent_guest.wasm")
  private val ToolMiddlewareGuestArtifact      = GuestArtifact("tool_middleware_guest.wasm")
  private val AgentToolMiddlewareGuestArtifact = GuestArtifact("agent_tool_middleware_guest.wasm")

  // ─── Private helpers ────────────────────────────────────────────────────────

  private def sha256(bytes: Array[Byte]): Array[Byte] = {
    val md = MessageDigest.getInstance("SHA-256")
    md.update(bytes)
    md.digest()
  }

  private def embeddedGuestWasmBytes(artifact: GuestArtifact): Array[Byte] = {
    val resourcePath = s"golem/wasm/${artifact.fileName}"
    Option(getClass.getClassLoader.getResourceAsStream(resourcePath)) match {
      case Some(in) =>
        val bos = new ByteArrayOutputStream()
        try {
          val buf = new Array[Byte](64 * 1024)
          var n   = in.read(buf)
          while (n >= 0) {
            if (n > 0) bos.write(buf, 0, n)
            n = in.read(buf)
          }
        } finally in.close()
        bos.toByteArray
      case None =>
        throw new RuntimeException(
          s"[golem] Missing embedded resource '$resourcePath'. This should be packaged in the zio-golem-mill plugin."
        )
    }
  }

  private def ensureGuestWasm(artifact: GuestArtifact, out: os.Path): (PathRef, Boolean) = {
    val bytes       = embeddedGuestWasmBytes(artifact)
    val expectedSha = sha256(bytes)
    val currentSha  = if (os.exists(out) && os.size(out) > 0) Some(sha256(os.read.bytes(out))) else None

    if (currentSha.exists(java.util.Arrays.equals(_, expectedSha))) (PathRef(out), false)
    else {
      os.makeDir.all(out / os.up)
      os.write.over(out, bytes)
      (PathRef(out), true)
    }
  }

  // ─── Settings ───────────────────────────────────────────────────────────────

  /** Base package whose `@agentImplementation` classes should be auto-registered. */
  def golemBasePackage: T[Option[String]] = Task(None)

  /**
   * Where the base guest runtime wasm should be written.
   *
   * Default: searches up from `moduleDir` for a `golem.yaml` containing an `app:` directive,
   * then places `agent_guest.wasm` in `.generated/` under that app root. Falls back to
   * `moduleDir / ".generated" / "agent_guest.wasm"`.
   */
  def golemAgentGuestWasmFile: os.Path = {
    @annotation.tailrec
    def findAppRoot(dir: os.Path): Option[os.Path] = {
      val manifest = dir / "golem.yaml"
      val isAppManifest =
        os.exists(manifest) && os.read(manifest).linesIterator.exists(_.trim.startsWith("app:"))
      if (isAppManifest) Some(dir)
      else if (dir == BuildCtx.workspaceRoot) None
      else findAppRoot(dir / os.up)
    }

    findAppRoot(moduleDir)
      .map(_ / ".generated" / AgentGuestArtifact.fileName)
      .getOrElse(moduleDir / ".generated" / AgentGuestArtifact.fileName)
  }

  /** Where the pure tool middleware guest runtime wasm should be written. */
  def golemToolMiddlewareGuestWasmFile: os.Path =
    golemAgentGuestWasmFile / os.up / ToolMiddlewareGuestArtifact.fileName

  /** Where the combined agent and tool middleware guest runtime wasm should be written. */
  def golemAgentToolMiddlewareGuestWasmFile: os.Path =
    golemAgentGuestWasmFile / os.up / AgentToolMiddlewareGuestArtifact.fileName

  // ─── Tasks ──────────────────────────────────────────────────────────────────

  /** Ensures the base guest runtime wasm exists; writes the embedded resource if missing or out-of-date. */
  def golemEnsureAgentGuestWasm(): Command[PathRef] = Task.Command {
    val (result, wrote) = ensureGuestWasm(AgentGuestArtifact, golemAgentGuestWasmFile)
    if (wrote) Task.log.info(s"[golem] Wrote embedded ${AgentGuestArtifact.fileName} to ${result.path}")
    result
  }

  /** Ensures the pure tool middleware guest runtime wasm exists and is up-to-date. */
  def golemEnsureToolMiddlewareGuestWasm(): Command[PathRef] = Task.Command {
    val (result, wrote) = ensureGuestWasm(ToolMiddlewareGuestArtifact, golemToolMiddlewareGuestWasmFile)
    if (wrote) Task.log.info(s"[golem] Wrote embedded ${ToolMiddlewareGuestArtifact.fileName} to ${result.path}")
    result
  }

  /** Ensures the combined agent and tool middleware guest runtime wasm exists and is up-to-date. */
  def golemEnsureAgentToolMiddlewareGuestWasm(): Command[PathRef] = Task.Command {
    val (result, wrote) =
      ensureGuestWasm(AgentToolMiddlewareGuestArtifact, golemAgentToolMiddlewareGuestWasmFile)
    if (wrote)
      Task.log.info(s"[golem] Wrote embedded ${AgentToolMiddlewareGuestArtifact.fileName} to ${result.path}")
    result
  }

  /**
   * Prepares the app directory for golem-cli by ensuring every role runtime exists and is up-to-date.
   */
  def golemPrepare(): Command[Unit] = Task.Command {
    golemEnsureAgentGuestWasm()
    golemEnsureToolMiddlewareGuestWasm()
    golemEnsureAgentToolMiddlewareGuestWasm()
    ()
  }

  /**
   * Builds the Scala.js bundle and writes it to the provided output path for golem-cli.
   *
   * Called by golem-cli during `golem build` via the command in `scala-js-template.yaml`:
   * {{{
   *   mill <module>.golemBuildComponent <component-name> <output-path>
   * }}}
   */
  def golemBuildComponent(component: String, outPath: String): Command[PathRef] = Task.Command {
    golemPrepare()
    Task.log.info(s"[golem] Building Scala.js bundle for $component ...")
    val report = fastLinkJS()
    val jsName =
      report.publicModules.headOption
        .map(_.jsFileName)
        .getOrElse(throw new RuntimeException("[golem] No public Scala.js modules were linked."))

    val jsFile = report.dest.path / jsName
    // outPath is typically absolute (from golem-cli's $COMP_DIR expansion), but handle relative too
    val out =
      if (outPath.startsWith("/")) os.Path(outPath)
      else BuildCtx.workspaceRoot / os.SubPath(outPath)

    os.makeDir.all(out / os.up)
    os.copy.over(jsFile, out)
    Task.log.info(s"[golem] Wrote Scala.js bundle to $out")
    PathRef(out)
  }

  // ─── Compiler options ────────────────────────────────────────────────────────

  /**
   * Makes Scaladoc comments of already-compiled sources visible to the
   * agent/tool macros (Symbol.docstring) by reading docs back from TASTy.
   */
  override def scalacOptions: T[Seq[String]] = Task {
    val docFlags = scalaVersion().split('.').toList match {
      case "3" :: minor :: _ if minor.forall(_.isDigit) && minor.toInt >= 8 => Seq("-Xread-docs")
      case "3" :: _                                                        => Seq("-Yread-docs")
      case _                                                               => Nil
    }
    super.scalacOptions() ++ docFlags
  }

  // ─── Module initializer auto-configuration ──────────────────────────────────

  override def moduleInitializers: Task[Seq[sjs.ModuleInitializer]] = Task.Anon {
    val base = super.moduleInitializers()
    golemBasePackage() match {
      case Some(basePackage) =>
        base ++ Seq(
          sjs.ModuleInitializer.mainMethod(s"${AutoRegisterCodegen.generatedPackage(basePackage)}.RegisterAgents", "main")
        )
      case None => base
    }
  }

  // ─── Auto-register source generation ────────────────────────────────────────

  private def golemSourceRoots = Task.Sources(moduleDir / "src")

  /** Generates Scala sources under `Task.dest` and returns them as generated sources. */
  def golemGeneratedAutoRegisterSources: T[Seq[PathRef]] = Task {
    val basePackageOpt = golemBasePackage()

    {
      val scalaSources: Seq[os.Path] =
        golemSourceRoots().flatMap(root => os.walk(root.path))
          .filter(p => os.isFile(p) && p.ext == "scala")

      val discoveryInputs = scalaSources.map { p =>
        SourceDiscovery.SourceInput(p.toString, os.read(p))
      }
      val discovered = SourceDiscovery.discover(discoveryInputs)

      def writePipelineFiles(files: Seq[CodegenPipeline.GeneratedFile], root: os.Path): Seq[os.Path] =
        files.map { gf =>
          val out = root / os.SubPath(gf.relativePath)
          os.makeDir.all(out / os.up)
          os.write.over(out, gf.content)
          out
        }

      def cleanStale(root: os.Path, validPaths: Set[os.Path]): Unit =
        if (os.exists(root))
          os.walk(root)
            .filter(path => os.isFile(path) && path.ext == "scala" && !validPaths.contains(path))
            .foreach { stale =>
              os.remove(stale)
              Task.log.debug(s"[golem] Removed stale generated file: $stale")
            }

      val pipeline = CodegenPipeline.run(discovered, basePackageOpt, rpcEnabled = true)

      // Auto-register generation
      val autoRegRoot = Task.dest / "golem" / "generated" / "autoregister"
      val autoRegPaths: Seq[os.Path] = pipeline.autoRegister match {
        case None =>
          cleanStale(autoRegRoot, Set.empty)
          Seq.empty
        case Some(ar) =>
          ar.warnings.foreach(w => Task.log.error(s"[golem] $w"))
          if (ar.files.isEmpty) {
            cleanStale(autoRegRoot, Set.empty)
            Seq.empty
          }
          else {
            val written = writePipelineFiles(ar.files, autoRegRoot)
            cleanStale(autoRegRoot, written.toSet)
            Task.log.info(
              s"[golem] Generated Scala.js component registration for ${basePackageOpt.get} into ${ar.generatedPackage} (${ar.implCount} impls, ${ar.packageCount} pkgs)."
            )
            written
          }
      }

      // RPC companion generation
      val rpcRoot = Task.dest / "golem" / "generated" / "rpc"
      val rpcPaths: Seq[os.Path] = {
        pipeline.rpc.warnings.foreach(w => Task.log.error(s"[golem] $w"))
        if (pipeline.rpc.files.isEmpty) {
          cleanStale(rpcRoot, Set.empty)
          Seq.empty
        }
        else {
          val written = writePipelineFiles(pipeline.rpc.files, rpcRoot)
          cleanStale(rpcRoot, written.toSet)
          Task.log.info(s"[golem] Generated ${pipeline.rpc.files.size} typed RPC and middleware source file(s).")
          written
        }
      }

      (autoRegPaths ++ rpcPaths).map(PathRef(_))
    }
  }

  override def generatedSources: T[Seq[PathRef]] =
    Task { super.generatedSources() ++ golemGeneratedAutoRegisterSources() }
}
