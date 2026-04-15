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

package golem.sbt

import sbt._
import sbt.Keys._
import sbt.complete.Parsers.spaceDelimited

import java.io.ByteArrayOutputStream
import java.io.FileOutputStream
import java.nio.file.Paths
import java.security.MessageDigest

import org.scalajs.linker.interface.{ModuleInitializer, Report}
import org.scalajs.sbtplugin.ScalaJSPlugin
import org.scalafmt.interfaces.Scalafmt

import golem.codegen.autoregister.AutoRegisterCodegen
import golem.codegen.discovery.SourceDiscovery
import golem.codegen.pipeline.CodegenPipeline

/**
 * sbt plugin for Golem-related build wiring.
 *
 * Currently this provides Scala.js agent auto-registration generation, so
 * user-land code never needs to write/maintain a `RegisterAgents` list.
 *
 * The plugin scans Scala sources for `@agentImplementation` classes and
 * generates an exported Scala.js entrypoint (`__golemRegisterAgents`) that
 * registers them.
 */
object GolemPlugin extends AutoPlugin {

  private lazy val scalafmt: Scalafmt = Scalafmt.create(getClass.getClassLoader)

  private def formatCode(content: String, configFile: File, fileName: String): String =
    scalafmt.format(configFile.toPath, Paths.get(fileName), content)

  private def sha256(bytes: Array[Byte]): Array[Byte] = {
    val md = MessageDigest.getInstance("SHA-256")
    md.update(bytes)
    md.digest()
  }

  private def sameSha256(file: File, expectedSha: Array[Byte]): Boolean =
    file.exists() && file.length() > 0 && java.util.Arrays.equals(sha256(IO.readBytes(file)), expectedSha)

  private def latestModified(file: File): Long =
    if (!file.exists()) 0L
    else if (file.isFile) file.lastModified()
    else (file ** "*").get.iterator.filter(_.isFile).map(_.lastModified()).foldLeft(0L)(math.max)

  private def latestModified(files: Iterable[File]): Long =
    files.foldLeft(0L)((currentMax, file) => math.max(currentMax, latestModified(file)))

  private def latestLinkedJsModified(outDir: File): Long =
    if (!outDir.exists()) 0L
    else (outDir ** "*.js").get.iterator.map(_.lastModified()).foldLeft(0L)(math.max)

  private def linkedJsFile(report: Report, outDir: File): File =
    report.publicModules.headOption
      .map(module => outDir / module.jsFileName)
      .getOrElse(sys.error("[golem] No public Scala.js modules were linked."))

  private def ensureFreshLinkedJs(jsFile: File, newestInputModified: Long): Unit =
    if (!jsFile.exists() || jsFile.lastModified() < newestInputModified) {
      sys.error(
        s"[golem] Scala.js linker output is stale at ${jsFile.getAbsolutePath}; refusing to copy it. Try running the build again or clean fullLinkJS first."
      )
    }

  private def embeddedAgentGuestWasmBytes(cl: ClassLoader, repoRootFallback: File): Array[Byte] = {
    val resourcePath = "golem/wasm/agent_guest.wasm"
    Option(cl.getResourceAsStream(resourcePath)) match {
      case Some(in) =>
        val bos = new ByteArrayOutputStream()
        try IO.transfer(in, bos)
        finally in.close()
        bos.toByteArray
      case None =>
        // Fallback for monorepo builds, where the plugin source is compiled into the meta-build.
        val candidate =
          repoRootFallback / "golem" / "sbt" / "src" / "main" / "resources" / "golem" / "wasm" / "agent_guest.wasm"
        if (candidate.exists()) IO.readBytes(candidate)
        else
          sys.error(
            s"[golem] Missing embedded resource '$resourcePath' (and no repo fallback at ${candidate.getAbsolutePath})."
          )
    }
  }

  object autoImport {
    val golemBasePackage: SettingKey[Option[String]] =
      settingKey[Option[String]](
        "Base package whose @agentImplementation classes should be auto-registered (Scala.js)."
      )

    val golemAgentGuestWasmFile: SettingKey[File] =
      settingKey[File](
        "Where to write the embedded base guest runtime WASM (agent_guest.wasm) for use by app manifests."
      )

    val golemWriteAgentGuestWasm: TaskKey[File] =
      taskKey[File]("Writes the embedded base guest runtime WASM (agent_guest.wasm) to golemAgentGuestWasmFile.")

    val golemEnsureAgentGuestWasm: TaskKey[File] =
      taskKey[File](
        "Ensures the base guest runtime WASM (agent_guest.wasm) exists at golemAgentGuestWasmFile; writes it if missing."
      )

    val golemPrepare: TaskKey[Unit] =
      taskKey[Unit](
        "Prepares the app directory for golem-cli by ensuring agent_guest.wasm exists and is up-to-date."
      )

    val golemBuildComponent: InputKey[File] =
      inputKey[File](
        "Builds the Scala.js bundle and writes it to the provided output path for golem-cli."
      )

  }

  import autoImport._

  override def requires: Plugins      = plugins.JvmPlugin && ScalaJSPlugin
  override def trigger: PluginTrigger = noTrigger

  override def projectSettings: Seq[Def.Setting[_]] =
    Seq(
      golemBasePackage        := None,
      golemAgentGuestWasmFile := {
        val projectRoot = (ThisProject / baseDirectory).value
        val buildRoot   = (ThisBuild / baseDirectory).value

        @annotation.tailrec
        def findAppRoot(dir: File): Option[File] =
          if (dir == null) None
          else {
            val manifest      = dir / "golem.yaml"
            val isAppManifest =
              manifest.exists() && IO.read(manifest).linesIterator.exists(line => line.trim.startsWith("app:"))
            if (isAppManifest) Some(dir) else findAppRoot(dir.getParentFile)
          }

        findAppRoot(projectRoot)
          .map(appRoot => appRoot / ".generated" / "agent_guest.wasm")
          .getOrElse {
            projectRoot / ".generated" / "agent_guest.wasm"
          }
      },
      golemWriteAgentGuestWasm := {
        val out = golemAgentGuestWasmFile.value
        val log = streams.value.log

        val repoRootFallback = (LocalRootProject / baseDirectory).value
        val bytes            = embeddedAgentGuestWasmBytes(getClass.getClassLoader, repoRootFallback)

        IO.createDirectory(out.getParentFile)

        val fos = new FileOutputStream(out)
        try {
          fos.write(bytes)
        } finally {
          fos.close()
        }

        log.info(s"[golem] Wrote embedded agent_guest.wasm to ${out.getAbsolutePath}")
        out
      },
      golemEnsureAgentGuestWasm := {
        Def.taskDyn {
          val out              = golemAgentGuestWasmFile.value
          val repoRootFallback = (LocalRootProject / baseDirectory).value
          val bytes            = embeddedAgentGuestWasmBytes(getClass.getClassLoader, repoRootFallback)
          val expectedSha      = sha256(bytes)

          if (sameSha256(out, expectedSha)) Def.task(out)
          else
            Def.task {
              val reason = if (!out.exists() || out.length() == 0) "missing" else "out-of-date"
              streams.value.log.info(
                s"[golem] agent_guest.wasm is $reason at ${out.getAbsolutePath}; writing embedded copy."
              )
              golemWriteAgentGuestWasm.value
            }
        }.value
      },
      golemPrepare := {
        golemEnsureAgentGuestWasm.value
        ()
      },
      golemBuildComponent := Def.inputTaskDyn {
        val args         = spaceDelimited("<component> <outFile> <agentWasmFile?>").parsed
        val component    = args.headOption.getOrElse(sys.error("Missing component name"))
        val outPath      = args.lift(1).getOrElse(".golem/scala.js")
        val agentWasmOpt = args.lift(2)
        Def.taskDyn {
          val out = file(outPath)
          val log = streams.value.log

          agentWasmOpt.foreach { p =>
            val target = file(p)
            val bytes  = embeddedAgentGuestWasmBytes(getClass.getClassLoader, (LocalRootProject / baseDirectory).value)
            val sha    = sha256(bytes)
            if (!sameSha256(target, sha)) {
              IO.createDirectory(target.getParentFile)
              val fos = new FileOutputStream(target)
              try fos.write(bytes)
              finally fos.close()
              log.info(s"[golem] Wrote embedded agent_guest.wasm to ${target.getAbsolutePath}")
            }
          }

          (Compile / compile).value

          val outDir =
            (Compile / ScalaJSPlugin.autoImport.fullLinkJS / ScalaJSPlugin.autoImport.scalaJSLinkerOutputDirectory).value
          val newestInputModified =
            latestModified((Compile / sources).value ++ (Compile / products).value)
          val staleLinkerOutputs = {
            val newestLinkedOutput = latestLinkedJsModified(outDir)
            newestLinkedOutput > 0L && newestLinkedOutput < newestInputModified
          }

          if (staleLinkerOutputs) {
            Def.task {
              log.warn(
                s"[golem] Detected stale Scala.js linker output in ${outDir.getAbsolutePath}; deleting it before relinking."
              )
              IO.delete(outDir)
              log.info(s"[golem] Building Scala.js bundle for $component ...")
              val report = (Compile / ScalaJSPlugin.autoImport.fullLinkJS).value.data
              val jsFile = linkedJsFile(report, outDir)
              ensureFreshLinkedJs(jsFile, newestInputModified)
              IO.createDirectory(out.getParentFile)
              IO.copyFile(jsFile, out, preserveLastModified = true)
              log.info(s"[golem] Wrote Scala.js bundle to ${out.getAbsolutePath}")
              out
            }
          } else {
            Def.task {
              log.info(s"[golem] Building Scala.js bundle for $component ...")
              val report = (Compile / ScalaJSPlugin.autoImport.fullLinkJS).value.data
              val jsFile = linkedJsFile(report, outDir)
              ensureFreshLinkedJs(jsFile, newestInputModified)
              IO.createDirectory(out.getParentFile)
              IO.copyFile(jsFile, out, preserveLastModified = true)
              log.info(s"[golem] Wrote Scala.js bundle to ${out.getAbsolutePath}")
              out
            }
          }
        }
      }.evaluated,
      Compile / compile                             := (Compile / compile).dependsOn(golemPrepare).value,
      Compile / ScalaJSPlugin.autoImport.fastLinkJS :=
        (Compile / ScalaJSPlugin.autoImport.fastLinkJS).dependsOn(golemPrepare).value,
      Compile / ScalaJSPlugin.autoImport.fullLinkJS :=
        (Compile / ScalaJSPlugin.autoImport.fullLinkJS).dependsOn(golemPrepare).value,
      Compile / ScalaJSPlugin.autoImport.scalaJSModuleInitializers ++= {
        golemBasePackage.value.toList.map { basePackage =>
          ModuleInitializer.mainMethod(s"${AutoRegisterCodegen.generatedPackage(basePackage)}.RegisterAgents", "main")
        }
      },
      Compile / sourceGenerators += Def.task {
        val basePackageOpt = golemBasePackage.value
        val log            = streams.value.log
        val managedBase    = (Compile / sourceManaged).value / "golem" / "generated"

        {
          // Shared discovery: scan sources once for both auto-register and RPC codegen
          val scalaSources =
            (Compile / unmanagedSourceDirectories).value
              .flatMap(dir => (dir ** "*.scala").get)
              .distinct

          val discoveryInputs = scalaSources.map { f =>
            SourceDiscovery.SourceInput(f.getAbsolutePath, IO.read(f))
          }
          val discovered = SourceDiscovery.discover(discoveryInputs)

          val scalafmtConfig = {
            @annotation.tailrec
            def findConfig(dir: File): File =
              if (dir == null) (LocalRootProject / baseDirectory).value / ".scalafmt.conf"
              else {
                val candidate = dir / ".scalafmt.conf"
                if (candidate.exists()) candidate else findConfig(dir.getParentFile)
              }
            findConfig((ThisProject / baseDirectory).value)
          }

          def writeIfChanged(out: File, content: String): Unit =
            if (!out.exists() || IO.read(out) != content) IO.write(out, content)

          def cleanStale(root: File, validPaths: Set[File]): Unit =
            if (root.exists()) {
              val existing = (root ** "*.scala").get.toSet
              (existing -- validPaths).foreach { stale =>
                IO.delete(stale)
                log.debug(s"[golem] Removed stale generated file: ${stale.getAbsolutePath}")
              }
            }

          def writePipelineFiles(files: Seq[CodegenPipeline.GeneratedFile], root: File): Seq[File] =
            files.map { gf =>
              val out       = root / gf.relativePath
              val formatted =
                try formatCode(gf.content, scalafmtConfig, out.getAbsolutePath)
                catch {
                  case e: Throwable =>
                    log.warn(
                      s"[golem] scalafmt failed for ${gf.relativePath}: ${e.getMessage}; using unformatted output"
                    )
                    gf.content
                }
              writeIfChanged(out, formatted)
              out
            }

          val pipeline = CodegenPipeline.run(discovered, basePackageOpt, rpcEnabled = true)

          // Auto-register generation
          val autoRegFiles: Seq[File] = pipeline.autoRegister match {
            case None     => Nil
            case Some(ar) =>
              ar.warnings.foreach(w => log.warn(s"[golem] $w"))
              val autoRegRoot = managedBase / "autoregister"
              if (ar.files.isEmpty) { cleanStale(autoRegRoot, Set.empty); Nil }
              else {
                val written = writePipelineFiles(ar.files, autoRegRoot)
                cleanStale(autoRegRoot, written.toSet)
                log.info(
                  s"[golem] Generated Scala.js agent registration for ${basePackageOpt.get} into ${ar.generatedPackage} (${ar.implCount} impls, ${ar.packageCount} pkgs)."
                )
                written
              }
          }

          // RPC companion generation
          val rpcFiles: Seq[File] = {
            pipeline.rpc.warnings.foreach(w => log.warn(s"[golem] $w"))
            val rpcRoot = managedBase / "rpc"
            if (pipeline.rpc.files.isEmpty) { cleanStale(rpcRoot, Set.empty); Nil }
            else {
              val written = writePipelineFiles(pipeline.rpc.files, rpcRoot)
              cleanStale(rpcRoot, written.toSet)
              log.info(s"[golem] Generated ${pipeline.rpc.files.size} RPC client object(s).")
              written
            }
          }

          autoRegFiles ++ rpcFiles
        }
      }.taskValue
    )
}
