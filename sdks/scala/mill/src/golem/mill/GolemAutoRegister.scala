package golem.mill

import mill.*
import mill.scalalib.*
import mill.scalajslib.*
import mill.scalajslib.api.ModuleInitializer

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
 *  - `scalaJSModuleInitializers` — auto-configured for the generated `RegisterAgents` entrypoint
 *
 * External usage (example):
 *
 * ```scala
 * import $ivy.`dev.zio::zio-golem-mill:<VERSION>`
 * import golem.mill.GolemAutoRegister
 *
 * object demo extends ScalaJSModule with GolemAutoRegister {
 *   def scalaJSVersion = "1.20.0"
 *   def scalaVersion   = "3.3.7"
 *   def golemBasePackage = T(Some("demo"))
 * }
 * ```
 */
trait GolemAutoRegister extends ScalaJSModule {

  // ─── Private helpers ────────────────────────────────────────────────────────

  private def sha256(bytes: Array[Byte]): Array[Byte] = {
    val md = MessageDigest.getInstance("SHA-256")
    md.update(bytes)
    md.digest()
  }

  private def embeddedAgentGuestWasmBytes(): Array[Byte] = {
    val resourcePath = "golem/wasm/agent_guest.wasm"
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

  // ─── Settings ───────────────────────────────────────────────────────────────

  /** Base package whose `@agentImplementation` classes should be auto-registered. */
  def golemBasePackage: T[Option[String]] = T(None)

  /**
   * Where the base guest runtime wasm should be written.
   *
   * Default: searches up from `millSourcePath` for a `golem.yaml` containing an `app:` directive,
   * then places `agent_guest.wasm` in `.generated/` under that app root. Falls back to
   * `millSourcePath / ".generated" / "agent_guest.wasm"`.
   */
  def golemAgentGuestWasmFile: T[os.Path] = T {
    @annotation.tailrec
    def findAppRoot(dir: os.Path): Option[os.Path] = {
      val manifest = dir / "golem.yaml"
      val isAppManifest =
        os.exists(manifest) && os.read(manifest).linesIterator.exists(_.trim.startsWith("app:"))
      if (isAppManifest) Some(dir)
      else {
        val parent = dir / os.up
        if (parent == dir) None // filesystem root
        else findAppRoot(parent)
      }
    }

    findAppRoot(millSourcePath)
      .map(_ / ".generated" / "agent_guest.wasm")
      .getOrElse(millSourcePath / ".generated" / "agent_guest.wasm")
  }

  // ─── Tasks ──────────────────────────────────────────────────────────────────

  /** Ensures the base guest runtime wasm exists; writes the embedded resource if missing or out-of-date. */
  def golemEnsureAgentGuestWasm: T[PathRef] = T {
    val out         = golemAgentGuestWasmFile()
    val bytes       = embeddedAgentGuestWasmBytes()
    val expectedSha = sha256(bytes)
    val currentSha  = if (os.exists(out) && os.size(out) > 0) Some(sha256(os.read.bytes(out))) else None

    if (currentSha.exists(java.util.Arrays.equals(_, expectedSha))) PathRef(out)
    else {
      os.makeDir.all(out / os.up)
      os.write.over(out, bytes)
      T.log.info(s"[golem] Wrote embedded agent_guest.wasm to $out")
      PathRef(out)
    }
  }

  /**
   * Prepares the app directory for golem-cli by ensuring agent_guest.wasm exists and is up-to-date.
   */
  def golemPrepare: T[Unit] = T {
    golemEnsureAgentGuestWasm()
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
  def golemBuildComponent(component: String, outPath: String): Command[PathRef] = T.command {
    T.log.info(s"[golem] Building Scala.js bundle for $component ...")
    val report = fastLinkJS()
    val jsName =
      report.publicModules.headOption
        .map(_.jsFileName)
        .getOrElse(throw new RuntimeException("[golem] No public Scala.js modules were linked."))

    val jsFile = report.dest.path / jsName
    // outPath is typically absolute (from golem-cli's $COMP_DIR expansion), but handle relative too
    val out =
      if (outPath.startsWith("/")) os.Path(outPath)
      else T.workspace / os.SubPath(outPath)

    os.makeDir.all(out / os.up)
    os.copy.over(jsFile, out)
    T.log.info(s"[golem] Wrote Scala.js bundle to $out")
    PathRef(out)
  }

  // ─── Module initializer auto-configuration ──────────────────────────────────

  override def scalaJSModuleInitializers: T[Seq[ModuleInitializer]] = T {
    val base = super.scalaJSModuleInitializers()
    golemBasePackage() match {
      case Some(basePackage) =>
        base ++ Seq(
          ModuleInitializer.mainMethod(s"${AutoRegisterCodegen.generatedPackage(basePackage)}.RegisterAgents", "main")
        )
      case None => base
    }
  }

  // ─── Auto-register source generation ────────────────────────────────────────

  /** Generates Scala sources under `T.dest` and returns them as generated sources. */
  def golemGeneratedAutoRegisterSources: T[Seq[PathRef]] = T {
    val basePackageOpt = golemBasePackage()

    {
      val scalaSources: Seq[os.Path] =
        os.walk(millSourcePath / "src")
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

      val pipeline = CodegenPipeline.run(discovered, basePackageOpt, rpcEnabled = true)

      // Auto-register generation
      val autoRegPaths: Seq[os.Path] = pipeline.autoRegister match {
        case None => Seq.empty
        case Some(ar) =>
          ar.warnings.foreach(w => T.log.error(s"[golem] $w"))
          if (ar.files.isEmpty) Seq.empty
          else {
            val written = writePipelineFiles(ar.files, T.dest / "golem" / "generated" / "autoregister")
            T.log.info(
              s"[golem] Generated Scala.js agent registration for ${basePackageOpt.get} into ${ar.generatedPackage} (${ar.implCount} impls, ${ar.packageCount} pkgs)."
            )
            written
          }
      }

      // RPC companion generation
      val rpcPaths: Seq[os.Path] = {
        pipeline.rpc.warnings.foreach(w => T.log.error(s"[golem] $w"))
        if (pipeline.rpc.files.isEmpty) Seq.empty
        else {
          val written = writePipelineFiles(pipeline.rpc.files, T.dest / "golem" / "generated" / "rpc")
          T.log.info(s"[golem] Generated ${pipeline.rpc.files.size} RPC client object(s).")
          written
        }
      }

      (autoRegPaths ++ rpcPaths).map(PathRef(_))
    }
  }

  // ─── Compile hooks ──────────────────────────────────────────────────────────

  override def compile: T[mill.scalalib.api.CompilationResult] = T {
    golemPrepare()
    super.compile()
  }

  override def generatedSources: T[Seq[PathRef]] =
    T { super.generatedSources() ++ golemGeneratedAutoRegisterSources() }
}
