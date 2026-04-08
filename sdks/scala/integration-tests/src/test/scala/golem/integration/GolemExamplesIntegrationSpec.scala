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

package golem.integration

import zio._
import zio.process.{Command as Cmd, Process as ZProcess, ProcessOutput}
import zio.test._

import com.sun.net.httpserver.{HttpExchange, HttpServer as JHttpServer}

import java.io.File
import java.net.InetSocketAddress
import java.net.URI
import java.net.http.{HttpClient, HttpRequest, HttpResponse}
import java.nio.channels.AsynchronousSocketChannel
import java.nio.file.Path
import java.time.Duration as JDuration

final case class GolemServer(process: ZProcess, examplesDir: File, tsPackagesPath: Option[String])

object GolemServer {

  private val golemPort        = 9881
  private val startupTimeout   = 60.seconds
  private val pollInterval     = 500.millis
  private val deployTimeoutSec = 300L

  private val examplesDir: File = {
    val cwd        = Path.of(sys.props.getOrElse("user.dir", ".")).toAbsolutePath.normalize
    val candidates = Seq(
      cwd.resolve("test-agents"),
      cwd.resolve("../test-agents"),
      cwd.resolve("golem/test-agents"),
      cwd.resolve("../golem/test-agents")
    ).map(_.normalize.toFile)

    candidates
      .find(d => new File(d, "golem.yaml").isFile)
      .getOrElse(sys.error(s"Could not locate test-agents dir (with golem.yaml) from user.dir=$cwd"))
  }

  private val tsPackagesPath: Option[String] =
    sys.props
      .get("golem.tsPackagesPath")
      .orElse(sys.env.get("GOLEM_TS_PACKAGES_PATH"))

  private def canConnect(port: Int): UIO[Boolean] =
    ZIO
      .acquireReleaseWith(
        ZIO.attemptBlockingIO(AsynchronousSocketChannel.open())
      )(ch => ZIO.succeed(ch.close())) { client =>
        ZIO
          .fromFutureJava(client.connect(new InetSocketAddress("localhost", port)))
          .as(true)
          .catchAll(_ => ZIO.succeed(false))
      }
      .catchAll(_ => ZIO.succeed(false))

  private def golemOnPath: ZIO[Any, Throwable, Unit] =
    Cmd("golem", "--version")
      .redirectErrorStream(true)
      .string
      .mapError(e => new RuntimeException(s"golem executable not found on PATH: $e"))
      .unit

  private def waitUntilReady(process: ZProcess): ZIO[Any, Throwable, Unit] = {
    val probe: ZIO[Any, Throwable, Unit] =
      process.isAlive.flatMap {
        case false =>
          ZIO.fail(new RuntimeException("golem server exited during startup"))
        case true =>
          canConnect(golemPort).flatMap {
            case true  => ZIO.unit
            case false => ZIO.fail(new RuntimeException("not ready yet"))
          }
      }

    probe
      .retry(Schedule.spaced(pollInterval) && Schedule.recurs(120))
      .timeoutFail(new RuntimeException(s"golem server did not become ready within $startupTimeout"))(startupTimeout)
  }

  private def buildEnv: Map[String, String] =
    tsPackagesPath.map(v => Map("GOLEM_TS_PACKAGES_PATH" -> v)).getOrElse(Map.empty)

  private def runGolemCmd(dir: File, timeoutSec: Long, args: String*): ZIO[Any, Throwable, GolemResult] = {
    val appManifest = new File(dir, "golem.yaml").getAbsolutePath
    val fullArgs    = Seq("--yes", "--local", "--app-manifest-path", appManifest) ++ args
    val label       = s"golem ${args.mkString(" ")}"

    Cmd("golem", fullArgs*)
      .workingDirectory(dir)
      .env(buildEnv)
      .redirectErrorStream(true)
      .run
      .flatMap { process =>
        for {
          output <- process.stdout.string
          code   <- process.exitCode
        } yield GolemResult(code.code, output)
      }
      .timeout(timeoutSec.seconds)
      .map {
        case Some(result) => result
        case None         => GolemResult(-1, s"TIMEOUT after ${timeoutSec}s")
      }
      .catchAll { e =>
        ZIO.succeed(GolemResult(-1, s"Command failed: $e"))
      }
      .tap { result =>
        ZIO.succeed {
          println(s"=== $label (exit=${result.exitCode}) ===")
          println(result.output)
          println(s"=== end $label ===")
        }
      }
  }

  private def deploy(dir: File): ZIO[Any, Throwable, Unit] =
    runGolemCmd(dir, deployTimeoutSec, "deploy").flatMap { result =>
      if (result.exitCode == 0) ZIO.unit
      else
        ZIO.sleep(2.seconds) *>
          runGolemCmd(dir, deployTimeoutSec, "deploy").flatMap { retry =>
            if (retry.exitCode == 0) ZIO.unit
            else
              ZIO.fail(
                new RuntimeException(
                  s"golem deploy failed after retry (exit=${retry.exitCode}):\n${retry.output}"
                )
              )
          }
    }

  private def provisionSecrets(dir: File): ZIO[Any, Throwable, Unit] = {
    val secretValues = Map(
      "apiKey"      -> """"test-api-key"""",
      "db.password" -> """"test-password""""
    )

    runGolemCmd(dir, 30L, "agent-secret", "list", "--format", "json").flatMap { listResult =>
      if (listResult.exitCode != 0)
        ZIO.fail(new RuntimeException(s"Failed to list secrets: ${listResult.output}"))
      else {
        import scala.util.matching.Regex

        val idPattern: Regex   = """"id"\s*:\s*"([^"]+)"""".r
        val pathPattern: Regex = """"path"\s*:\s*\[([^\]]*)\]""".r

        // Parse each secret block from the JSON array
        val secretBlocks = listResult.output.split("""\{""").tail.map("{" + _)
        ZIO.foreachDiscard(secretBlocks) { block =>
         val idOpt   = idPattern.findFirstMatchIn(block).map(_.group(1))
         val pathOpt = pathPattern.findFirstMatchIn(block).map(_.group(1))

         (idOpt, pathOpt) match {
           case (Some(id), Some(pathStr)) =>
             val normalizedPath = pathStr.replaceAll(""""|\s""", "").split(",").mkString(".")
             secretValues.get(normalizedPath) match {
               case Some(value) =>
                 runGolemCmd(
                   dir,
                   30L,
                   "agent-secret",
                   "update-value",
                   "--id",
                   id,
                   "--secret-value",
                   value
                  ).flatMap { result =>
                    if (result.exitCode == 0) ZIO.unit
                    else
                      ZIO.fail(
                        new RuntimeException(
                          s"Failed to update secret '$normalizedPath' (exit=${result.exitCode}):\n${result.output}"
                        )
                      )
                  }
                case None => ZIO.unit
              }
            case _ => ZIO.unit
          }
        }
      }
    }
  }

  val layer: ZLayer[Any, Throwable, GolemServer] =
    ZLayer.scoped {
      for {
        _ <- golemOnPath
        _ <- ZIO.when(tsPackagesPath.isEmpty)(
               ZIO.fail(
                 new RuntimeException(
                   "GOLEM_TS_PACKAGES_PATH env var or golem.tsPackagesPath system property must be set"
                 )
               )
             )
        _ <- canConnect(golemPort).flatMap {
               case false => ZIO.unit
               case true  => ZIO.fail(new RuntimeException(s"port $golemPort is already in use"))
             }
        // Clean stale REPL caches so regenerated scripts/SDKs are picked up
        _ <- ZIO.attemptBlocking {
               val golemTemp = new File(examplesDir, "golem-temp")
               if (golemTemp.exists()) {
                 def deleteRecursive(f: File): Unit = {
                   val path = f.toPath
                   if (java.nio.file.Files.isSymbolicLink(path)) {
                     java.nio.file.Files.delete(path)
                   } else if (f.isDirectory) {
                     Option(f.listFiles()).getOrElse(Array.empty[File]).foreach(deleteRecursive)
                     f.delete()
                   } else {
                     f.delete()
                   }
                 }
                 deleteRecursive(golemTemp)
               }
             }
        logFile <- ZIO.attemptBlocking {
                     java.nio.file.Files.createTempFile("golem-server-", ".log").toFile
                   }
        process <- ZIO.acquireRelease(
                     Cmd("golem", "-vvv", "server", "run", "--clean", "--disable-app-manifest-discovery")
                       .workingDirectory(examplesDir)
                       .env(buildEnv)
                       .redirectErrorStream(true)
                       .stdout(ProcessOutput.FileRedirect(logFile))
                       .run
                       .mapError(e => new RuntimeException(s"Failed to start golem server: $e"))
                   )(process => process.killTree.orElse(process.killTreeForcibly).ignore)
        _     <- waitUntilReady(process)
        server = GolemServer(process, examplesDir, tsPackagesPath)
        _     <- deploy(examplesDir)
        _     <- provisionSecrets(examplesDir)
      } yield server
    }
}

case class GolemResult(exitCode: Int, output: String)

object GolemExamplesIntegrationSpec extends ZIOSpec[GolemServer] {

  override val bootstrap: ZLayer[Any, Any, GolemServer] =
    GolemServer.layer

  private val replTimeoutSec = 180L

  // ---------------------------------------------------------------------------
  // Sample manifest
  // ---------------------------------------------------------------------------

  private case class Sample(
    name: String,
    script: String,
    assertion: Assertion,
    skip: Option[String] = None
  )

  private sealed trait Assertion
  private case class Contains(fragments: String*)        extends Assertion
  private case class Custom(check: String => TestResult) extends Assertion

  private val samples: Seq[Sample] = Seq(
    // --- Simple / deterministic ---
    Sample(
      "snapshot-counter",
      "samples/snapshot-counter/repl-snapshot-counter.ts",
      Custom { output =>
        assertTrue(
          output.contains("a:") || output.contains("a ="),
          output.contains("1"),
          output.contains("2"),
          output.contains("3")
        )
      }
    ),
    Sample(
      "auto-snapshot-counter",
      "samples/snapshot-counter-auto/repl-auto-snapshot-counter.ts",
      Custom { output =>
        assertTrue(
          output.contains("a:") || output.contains("a ="),
          output.contains("1"),
          output.contains("2"),
          output.contains("3")
        )
      }
    ),
    Sample(
      "fork",
      "samples/fork/repl-fork.ts",
      Contains("original-joined")
    ),
    Sample(
      "fork-json",
      "samples/fork/repl-fork-json.ts",
      Contains("original-joined-json: count=42")
    ),
    Sample(
      "sync-return",
      "samples/sync-return/repl-sync-return.ts",
      Contains("hello, world", "sum=7", "tag=test-tag")
    ),
    Sample(
      "json-tasks",
      "samples/json-tasks/repl-json-tasks.ts",
      Custom { output =>
        assertTrue(
          output.contains("t1"),
          output.contains("true") || output.contains("completed")
        )
      }
    ),
    Sample(
      "human-in-the-loop",
      "samples/human-in-the-loop/repl-human-in-the-loop.ts",
      Custom { output =>
        assertTrue(
          output.contains("pending"),
          output.contains("approved")
        )
      }
    ),
    Sample(
      "simple-rpc",
      "samples/simple-rpc/repl-counter.ts",
      Custom { output =>
        assertTrue(
          output.contains("olleh"),
          output.contains("dlrow")
        )
      }
    ),
    Sample(
      "agent-to-agent",
      "samples/agent-to-agent/repl-minimal-agent-to-agent.ts",
      Custom { output =>
        assertTrue(
          output.contains("olleh"),
          output.contains("cba")
        )
      }
    ),
    Sample(
      "stateful-counter",
      "samples/stateful-counter/repl-stateful-counter.ts",
      Custom { output =>
        assertTrue(
          output.contains("first") && output.contains("11"),
          output.contains("second") && output.contains("12"),
          output.contains("current") && output.contains("12")
        )
      }
    ),
    Sample(
      "shard",
      "samples/shard/repl-shard.ts",
      Contains("users:0:alice")
    ),
    Sample(
      "trigger",
      "samples/trigger/repl-trigger.ts",
      Custom { output =>
        assertTrue(
          output.contains("pong"),
          output.contains("10")
        )
      }
    ),

    // --- Transactions (deterministic trace output) ---
    Sample(
      "transactions-infallible",
      "samples/transactions/repl-infallible.ts",
      Contains("Infallible Transaction Demo", "transaction result=30")
    ),
    Sample(
      "transactions-fallible-success",
      "samples/transactions/repl-fallible-success.ts",
      Contains("transaction result=Right(51)")
    ),
    Sample(
      "transactions-fallible-failure",
      "samples/transactions/repl-fallible-failure.ts",
      Contains("FailedAndRolledBackCompletely", "intentional-failure")
    ),

    // --- Guards ---
    Sample(
      "guards-block",
      "samples/guards/repl-guards-block.ts",
      Contains("retry-ok", "level-ok", "idem-ok", "atomic-ok")
    ),
    Sample(
      "guards-resource",
      "samples/guards/repl-guards-resource.ts",
      Contains("Resource-style Guards Demo", "after drop():", "markAtomicOperation:")
    ),
    Sample(
      "guards-oplog",
      "samples/guards/repl-oplog.ts",
      Contains("current oplog index=", "markBeginOperation", "markEndOperation")
    ),

    // --- Observability ---
    Sample(
      "observability-trace",
      "samples/observability/repl-observability.ts",
      Contains("=== Trace Demo ===", "traceId", "spanId")
    ),
    Sample(
      "observability-durability",
      "samples/observability/repl-durability.ts",
      Contains("=== Durability Demo ===", "isLive", "persistenceLevel")
    ),

    // --- Storage / Config ---
    Sample(
      "storage-config",
      "samples/storage/repl-storage.ts",
      Contains("=== Config Demo ===", "Config.get")
    ),

    // --- JSON promise ---
    Sample(
      "json-promise",
      "samples/json-promise/repl-json-promise.ts",
      Contains("roundtrip", "createPromise ok")
    ),

    // --- Oplog inspector ---
    Sample(
      "oplog-inspector",
      "samples/oplog-inspector/repl-oplog-inspector.ts",
      Contains("=== Oplog Inspector")
    ),
    Sample(
      "oplog-search",
      "samples/oplog-inspector/repl-oplog-search.ts",
      Contains("=== Searching oplog")
    ),

    // --- Agent registry ---
    Sample(
      "agent-registry",
      "samples/agent-registry/repl-registry.ts",
      Contains("registeredAgentType", "getAllAgentTypes")
    ),
    Sample(
      "agent-registry-query",
      "samples/agent-registry/repl-agent-query.ts",
      Contains("agentType=", "agentName=")
    ),
    Sample(
      "agent-registry-phantom",
      "samples/agent-registry/repl-phantom.ts",
      Contains("phantom counter")
    ),

    // --- Host API explorer ---
    Sample(
      "host-api-explorer-all",
      "samples/host-api-explorer/repl-explore-all.ts",
      Contains("=== CONFIG", "=== DURABILITY", "=== CONTEXT")
    ),
    Sample(
      "host-api-explorer-config",
      "samples/host-api-explorer/repl-explore-config.ts",
      Contains("Config.get", "Config.getAll")
    ),
    Sample(
      "host-api-explorer-context",
      "samples/host-api-explorer/repl-explore-context.ts",
      Contains("traceId =", "spanId =")
    ),
    Sample(
      "host-api-explorer-durability",
      "samples/host-api-explorer/repl-explore-durability.ts",
      Contains("isLive=", "persistenceLevel=")
    ),
    Sample(
      "host-api-explorer-blobstore",
      "samples/host-api-explorer/repl-explore-blobstore.ts",
      Contains("containerExists", "createContainer")
    ),
    Sample(
      "host-api-explorer-oplog",
      "samples/host-api-explorer/repl-explore-oplog.ts",
      Contains("OplogApi.GetOplog entries=")
    ),
    Sample(
      "host-api-explorer-keyvalue",
      "samples/host-api-explorer/repl-explore-keyvalue.ts",
      Contains("set(", "get(", "exists(")
    ),
    Sample(
      "host-api-explorer-rdbms",
      "samples/host-api-explorer/repl-explore-rdbms.ts",
      Contains("Left(")
    ),

    // --- Config ---
    Sample(
      "config-default",
      "samples/config/repl-config-default.ts",
      Custom { output =>
        assertTrue(
          output.contains("config-default="),
          output.contains("ManifestApp"),
          output.contains("manifest-db.example.com"),
          output.contains("5432")
        )
      }
    ),
    Sample(
      "config-override",
      "samples/config/repl-config-override.ts",
      Custom { output =>
        assertTrue(
          output.contains("config-result="),
          output.contains("OverriddenApp"),
          output.contains("overridden-host.example.com"),
          output.contains("9999")
        )
      }
    ),

    // --- Principal injection ---
    Sample(
      "principal",
      "samples/principal/principal.ts",
      Contains("was created by:")
    ),

    // --- Database (requires external DB) ---
    Sample(
      "database",
      "samples/database/repl-database.ts",
      Contains("=== Type Showcase ===", "DbDate", "IpAddress"),
      skip = Some("requires database server (set RUN_DATABASE_TESTS=1)")
    )
  )

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  private def runGolem(server: GolemServer, timeoutSec: Long, args: String*): ZIO[Any, Nothing, GolemResult] = {
    val appManifest = new File(server.examplesDir, "golem.yaml").getAbsolutePath
    val fullArgs    = Seq("--yes", "-vvv", "--local", "--app-manifest-path", appManifest) ++ args
    val env         = server.tsPackagesPath.map(v => Map("GOLEM_TS_PACKAGES_PATH" -> v)).getOrElse(Map.empty)

    Cmd("golem", fullArgs*)
      .workingDirectory(server.examplesDir)
      .env(env)
      .redirectErrorStream(true)
      .string
      .timeout(timeoutSec.seconds)
      .map {
        case Some(output) => GolemResult(0, output)
        case None         => GolemResult(-1, s"TIMEOUT after ${timeoutSec}s")
      }
      .catchAll { e =>
        ZIO.succeed(GolemResult(-1, s"Command failed: $e"))
      }
  }

  private def runRepl(server: GolemServer, scriptFile: String): ZIO[Any, Nothing, GolemResult] =
    runGolem(
      server,
      replTimeoutSec,
      "repl",
      "scala:examples",
      "--language",
      "typescript",
      "--script-file",
      scriptFile
    )

  private def normalizeOutput(raw: String): String =
    raw
      .replaceAll("\u001b\\[[0-9;]*[a-zA-Z]", "")
      .replaceAll("\r\n", "\n")
      .trim

  private def findTsFiles(dir: File): Seq[File] =
    if (!dir.exists()) Seq.empty
    else {
      val files: Array[File] = Option(dir.listFiles()).getOrElse(Array.empty[File])
      val tsFiles            = files.filter(f => f.isFile && f.getName.endsWith(".ts"))
      val subdirs            = files.filter(_.isDirectory).flatMap(findTsFiles)
      tsFiles.toSeq ++ subdirs
    }

  private implicit class FileOps(f: File) {
    def relativeTo(base: File): String =
      base.toPath.relativize(f.toPath).toString
  }

  // ---------------------------------------------------------------------------
  // Test generation
  // ---------------------------------------------------------------------------

  private val sampleTests: Seq[Spec[GolemServer, Throwable]] = samples.map { sample =>
    test(sample.name) {
      for {
        server <- ZIO.service[GolemServer]
        result <- runRepl(server, sample.script)
      } yield {
        val exitCodeResult = assertTrue(result.exitCode == 0)
        val output         = normalizeOutput(result.output)

        val assertionResult = sample.assertion match {
          case Contains(fragments*) =>
            fragments.foldLeft(assertCompletes) { (acc, frag) =>
              acc && assertTrue(output.contains(frag))
            }
          case Custom(check) =>
            check(output)
        }

        exitCodeResult && assertionResult
      }
    } @@ TestAspect.timeout(300.seconds) @@ (if (sample.skip.isDefined) TestAspect.ignore else TestAspect.identity)
  }

  private val manifestTest: Spec[GolemServer, Throwable] =
    test("manifest covers all sample scripts") {
      for {
        server <- ZIO.service[GolemServer]
      } yield {
        val samplesDir = new File(server.examplesDir, "samples")
        val allScripts = findTsFiles(samplesDir).map(_.relativeTo(server.examplesDir)).sorted
        val manifest   = samples.map(_.script).sorted
        val uncovered  = allScripts.filterNot(manifest.contains)
        assertTrue(uncovered.isEmpty)
      }
    }

  // ---------------------------------------------------------------------------
  // HTTP endpoint tests (code-first HTTP routes)
  // ---------------------------------------------------------------------------

  private val httpPort = 9006

  private def httpGet(path: String): ZIO[Any, Throwable, (Int, String)] =
    ZIO.attemptBlocking {
      val client  = HttpClient.newBuilder().connectTimeout(JDuration.ofSeconds(10)).build()
      val request = HttpRequest
        .newBuilder()
        .uri(URI.create(s"http://localhost:$httpPort$path"))
        .GET()
        .timeout(JDuration.ofSeconds(30))
        .build()
      val response = client.send(request, HttpResponse.BodyHandlers.ofString())
      (response.statusCode(), response.body())
    }

  private def httpPost(path: String, body: String): ZIO[Any, Throwable, (Int, String)] =
    ZIO.attemptBlocking {
      val client  = HttpClient.newBuilder().connectTimeout(JDuration.ofSeconds(10)).build()
      val request = HttpRequest
        .newBuilder()
        .uri(URI.create(s"http://localhost:$httpPort$path"))
        .POST(HttpRequest.BodyPublishers.ofString(body))
        .header("Content-Type", "application/json")
        .timeout(JDuration.ofSeconds(30))
        .build()
      val response = client.send(request, HttpResponse.BodyHandlers.ofString())
      (response.statusCode(), response.body())
    }

  private def httpPostWithHeaders(
    path: String,
    body: String,
    headers: Map[String, String]
  ): ZIO[Any, Throwable, (Int, String)] =
    ZIO.attemptBlocking {
      val client  = HttpClient.newBuilder().connectTimeout(JDuration.ofSeconds(10)).build()
      val builder = HttpRequest
        .newBuilder()
        .uri(URI.create(s"http://localhost:$httpPort$path"))
        .POST(HttpRequest.BodyPublishers.ofString(body))
        .header("Content-Type", "application/json")
        .timeout(JDuration.ofSeconds(30))
      headers.foreach { case (k, v) => builder.header(k, v) }
      val response = client.send(builder.build(), HttpResponse.BodyHandlers.ofString())
      (response.statusCode(), response.body())
    }

  private def httpPostAbsolute(url: String, body: String): ZIO[Any, Throwable, (Int, String)] =
    ZIO.attemptBlocking {
      val client  = HttpClient.newBuilder().connectTimeout(JDuration.ofSeconds(10)).build()
      val request = HttpRequest
        .newBuilder()
        .uri(URI.create(url))
        .POST(HttpRequest.BodyPublishers.ofString(body))
        .header("Content-Type", "application/json")
        .timeout(JDuration.ofSeconds(30))
        .build()
      val response = client.send(request, HttpResponse.BodyHandlers.ofString())
      (response.statusCode(), response.body())
    }

  // ---------------------------------------------------------------------------
  // WeatherAgent: single-param Constructor (value: String)
  // Mount: /api/weather/{value}  — {value} is the default name for single-element types
  // ---------------------------------------------------------------------------
  private val weatherTests: Seq[Spec[GolemServer, Throwable]] = Seq(
    test("http-weather-get") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/weather/test-key/current/london")
      } yield assertTrue(status == 200) && assertTrue(body.contains("Sunny in london"))
    },

    test("http-weather-root") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/weather/test-key/")
      } yield assertTrue(status == 200) && assertTrue(body.contains("Welcome to the Weather API"))
    },

    test("http-weather-search") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/weather/test-key/search?q=rain&limit=5")
      } yield assertTrue(status == 200) && assertTrue(body.contains("5") && body.contains("rain"))
    },

    test("http-weather-catch-all") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/weather/test-key/greet/alice/some/nested/path")
      } yield assertTrue(status == 200) && assertTrue(body.contains("alice") && body.contains("some/nested/path"))
    },

    test("http-weather-header") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpPostWithHeaders(
                            "/api/weather/test-key/report",
                            """{"data": "weather data"}""",
                            Map("X-Tenant" -> "acme-corp")
                          )
      } yield assertTrue(status == 200) && assertTrue(body.contains("acme-corp"))
    },

    test("http-weather-public") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/weather/test-key/public")
      } yield assertTrue(status == 200) && assertTrue(body.contains("Public"))
    }
  ).map(_ @@ TestAspect.timeout(60.seconds))

  // ---------------------------------------------------------------------------
  // InventoryAgent: multi-param Constructor (arg0: String, arg1: Int)
  // Mount: /api/inventory/{arg0}/{arg1}  — positional names for tuple elements
  // ---------------------------------------------------------------------------
  private val inventoryTests: Seq[Spec[GolemServer, Throwable]] = Seq(
    test("http-inventory-stock") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/inventory/warehouse-a/3/stock")
      } yield assertTrue(status == 200) && assertTrue(body.contains("warehouse-a") && body.contains("3"))
    },

    test("http-inventory-item") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/inventory/warehouse-b/7/item/widget-99")
      } yield assertTrue(status == 200) && assertTrue(body.contains("widget-99") && body.contains("warehouse-b"))
    }
  ).map(_ @@ TestAspect.timeout(60.seconds))

  // ---------------------------------------------------------------------------
  // CatalogAgent: multi-param Constructor (region: String, catalog: String)
  // Mount: /api/catalog/{region}/{catalog}  — field names from case class
  // ---------------------------------------------------------------------------
  private val catalogTests: Seq[Spec[GolemServer, Throwable]] = Seq(
    test("http-catalog-search") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/catalog/us-east/electronics/search?q=laptop")
      } yield assertTrue(status == 200) && assertTrue(
        body.contains("electronics") && body.contains("us-east") && body.contains("laptop")
      )
    },

    test("http-catalog-item") {
      for {
        _              <- ZIO.service[GolemServer]
        (status, body) <- httpGet("/api/catalog/us-east/electronics/item/prod-123")
      } yield assertTrue(status == 200) && assertTrue(
        body.contains("prod-123") && body.contains("electronics") && body.contains("us-east")
      )
    }
  ).map(_ @@ TestAspect.timeout(60.seconds))

  // ---------------------------------------------------------------------------
  // WebhookDemo: webhook creation and payload await
  // Mount: /api/webhook-demo/{value}  — webhookSuffix: /incoming
  // ---------------------------------------------------------------------------
  private val webhookTests: Seq[Spec[GolemServer, Throwable]] = Seq(
    test("http-webhook-create-and-await") {
      for {
        _                             <- ZIO.service[GolemServer]
        (createStatus, rawWebhookUrl) <- httpGet("/api/webhook-demo/test-key/create")
        webhookUrl                     = rawWebhookUrl.trim.stripPrefix("\"").stripSuffix("\"")
        _                             <- ZIO.succeed(assertTrue(createStatus == 200))
        _                             <- ZIO.succeed(assertTrue(webhookUrl.contains("/incoming")))
        // POST JSON payload to the webhook URL — run concurrently with await
        awaitFiber               <- httpGet("/api/webhook-demo/test-key/await").fork
        _                        <- ZIO.sleep(1.second)
        (postStatus, _)          <- httpPostAbsolute(webhookUrl, """{"message":"hello","count":42}""")
        _                        <- ZIO.succeed(assertTrue(postStatus >= 200 && postStatus < 300))
        (awaitStatus, awaitBody) <- awaitFiber.join
      } yield assertTrue(awaitStatus == 200) &&
        assertTrue(awaitBody.contains("message=hello")) &&
        assertTrue(awaitBody.contains("count=42"))
    }
  ).map(_ @@ TestAspect.timeout(120.seconds))

  // ---------------------------------------------------------------------------
  // FetchAgent: outgoing HTTP requests via global fetch
  // ---------------------------------------------------------------------------

  private def withTestHttpServer[A](
    handler: HttpExchange => Unit
  )(body: Int => ZIO[Any, Throwable, A]): ZIO[Any, Throwable, A] =
    ZIO.acquireReleaseWith(
      ZIO.attemptBlocking {
        val server = JHttpServer.create(new InetSocketAddress("0.0.0.0", 0), 0)
        server.createContext(
          "/test",
          (exchange: HttpExchange) => {
            handler(exchange)
          }
        )
        server.setExecutor(null)
        server.start()
        server
      }
    )(server => ZIO.succeed(server.stop(0))) { server =>
      body(server.getAddress.getPort)
    }

  private val fetchTests: Seq[Spec[GolemServer, Throwable]] = Seq(
    test("http-fetch-outgoing") {
      for {
        _      <- ZIO.service[GolemServer]
        result <- withTestHttpServer { exchange =>
                    val response = "hello from test server"
                    exchange.sendResponseHeaders(200, response.length.toLong)
                    val os = exchange.getResponseBody
                    os.write(response.getBytes("UTF-8"))
                    os.close()
                  } { port =>
                    httpGet(s"/api/fetch/test-key/call?port=$port")
                  }
        (status, body) = result
      } yield assertTrue(status == 200) && assertTrue(body.contains("hello from test server"))
    }
  ).map(_ @@ TestAspect.timeout(60.seconds))

  private val httpTests: Seq[Spec[GolemServer, Throwable]] =
    weatherTests ++ inventoryTests ++ catalogTests ++ webhookTests ++ fetchTests

  // ---------------------------------------------------------------------------
  // Snapshotting oplog tests
  // ---------------------------------------------------------------------------

  private def queryOplog(server: GolemServer, agentId: String): ZIO[Any, Nothing, GolemResult] =
    runGolem(server, 30L, "agent", "oplog", agentId, "--format", "json")

  private val snapshotOplogTests: Seq[Spec[GolemServer, Throwable]] = Seq(
    test("snapshot-oplog-custom: custom saveSnapshot/loadSnapshot produces snapshot entries in oplog") {
      for {
        server      <- ZIO.service[GolemServer]
        oplogResult <- queryOplog(server, "SnapshotCounter(\"custom-demo\")")
      } yield {
        val output = normalizeOutput(oplogResult.output)
        assertTrue(oplogResult.exitCode == 0) &&
        assertTrue(output.contains("\"type\":\"Snapshot\""))
      }
    },
    test("snapshot-oplog-auto: Snapshotted[S] produces JSON snapshot entries in oplog") {
      for {
        server      <- ZIO.service[GolemServer]
        oplogResult <- queryOplog(server, "AutoSnapshotCounter(\"auto-demo\")")
      } yield {
        val output = normalizeOutput(oplogResult.output)
        assertTrue(oplogResult.exitCode == 0) &&
        assertTrue(output.contains("\"type\":\"Snapshot\"")) &&
        assertTrue(output.contains("\"type\":\"Json\"")) &&
        assertTrue(output.contains("\"value\":3"))
      }
    }
  ).map(_ @@ TestAspect.timeout(60.seconds))

  override def spec: Spec[GolemServer, Any] =
    suite("GolemExamplesIntegrationSpec")(
      (sampleTests ++ httpTests ++ snapshotOplogTests :+ manifestTest)*
    ) @@ TestAspect.sequential @@ TestAspect.withLiveClock
}
