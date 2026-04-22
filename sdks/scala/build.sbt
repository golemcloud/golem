import sbt.*
import sbt.Keys.*
import org.portablescala.sbtplatformdeps.PlatformDepsPlugin.autoImport.*
import sbtcrossproject.CrossPlugin.autoImport.*
import scalajscrossproject.ScalaJSCrossPlugin.autoImport.*

// ---------------------------------------------------------------------------
// Scala versions
// ---------------------------------------------------------------------------

val Scala3Golem = "3.8.2"
val Scala213    = "2.13.18"
val Scala212    = "2.12.21"

// ---------------------------------------------------------------------------
// Global settings
// ---------------------------------------------------------------------------

ThisBuild / organization     := "cloud.golem"
ThisBuild / scalaVersion     := Scala3Golem
ThisBuild / dynverTagPrefix  := "golem-scala-v"
ThisBuild / licenses     := List("Apache-2.0" -> url("https://www.apache.org/licenses/LICENSE-2.0"))
ThisBuild / homepage     := Some(url("https://github.com/golemcloud/golem"))
ThisBuild / scmInfo := Some(
  ScmInfo(
    url("https://github.com/golemcloud/golem"),
    "scm:git@github.com:golemcloud/golem.git"
  )
)
ThisBuild / developers := List(
  Developer("vigoo", "Daniel Vigovszky", "daniel.vigovszky@gmail.com", url("https://github.com/vigoo")),
  Developer("jdegoes", "John De Goes", "john@degoes.net", url("https://github.com/jdegoes"))
)

Global / onChangedBuildSource := ReloadOnSourceChanges

// ---------------------------------------------------------------------------
// Dependency versions
// ---------------------------------------------------------------------------

val ujsonVersion              = "3.1.0"
val scalametaVersion          = "4.14.7"
val munitVersion              = "1.1.0"
val zioTestVersion            = "2.1.24"
val zioSchemaDerivationVersion = "1.8.3"
val scalaJavaTimeVersion      = "2.6.0"
val zioHttpVersion            = "3.0.1"
val zioProcessVersion         = "0.8.0"
val scalafmtDynamicVersion    = "3.10.4"

// ---------------------------------------------------------------------------
// zio-blocks dependency helper
// ---------------------------------------------------------------------------

val zioBlocksVersion = "0.0.32"

def zioBlocksDep(name: String) = Def.setting {
  "dev.zio" %%% s"zio-blocks-$name" % zioBlocksVersion
}

def zioBlocksDepJvm(name: String) = Def.setting {
  "dev.zio" %% s"zio-blocks-$name" % zioBlocksVersion
}

// ---------------------------------------------------------------------------
// Common settings
// ---------------------------------------------------------------------------

lazy val commonSettings = Seq(
  testFrameworks += new TestFramework("zio.test.sbt.ZTestFramework"),
  scalacOptions ++= {
    CrossVersion.partialVersion(scalaVersion.value) match {
      case Some((3, minor)) if minor >= 5 => Seq("-experimental")
      case _                              => Nil
    }
  }
)

def versionSpecificSourceDirs(conf: Configuration) = Seq(
  conf / unmanagedSourceDirectories ++= {
    val base = (conf / sourceDirectory).value
    CrossVersion.partialVersion(scalaVersion.value) match {
      case Some((2, _)) => Seq(base / "scala-2")
      case Some((3, _)) => Seq(base / "scala-3")
      case _            => Nil
    }
  }
)

lazy val jsSettings = Seq(
  Test / parallelExecution := false
)

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

lazy val root = (project in file("."))
  .aggregate(
    model.jvm,
    model.js,
    core,
    macros,
    codegen,
    sbtPlugin,
    testAgents,
    integrationTests
  )
  .settings(
    name           := "golem-scala",
    publish / skip := true
  )

// --- model (cross JVM + JS, CrossType.Pure) --------------------------------

lazy val model = crossProject(JVMPlatform, JSPlatform)
  .crossType(CrossType.Pure)
  .in(file("model"))
  .settings(commonSettings)
  .settings(
    name               := "golem-scala-model",
    crossScalaVersions := Seq(Scala3Golem, Scala213),
    libraryDependencies ++= Seq(
      zioBlocksDep("schema").value,
      "dev.zio" %%% "zio-test"     % zioTestVersion % Test,
      "dev.zio" %%% "zio-test-sbt" % zioTestVersion % Test
    )
  )
  .settings(versionSpecificSourceDirs(Compile))
  .settings(versionSpecificSourceDirs(Test))
  .jvmSettings(
    Compile / unmanagedSourceDirectories ++= Seq(
      (ThisBuild / baseDirectory).value / "model" / ".jvm" / "src" / "main" / "scala"
    ),
    Test / unmanagedSourceDirectories ++= Seq(
      (ThisBuild / baseDirectory).value / "model" / ".jvm" / "src" / "test" / "scala"
    ),
    libraryDependencies ++= Seq(
      "com.lihaoyi" %% "ujson"                 % ujsonVersion,
      "dev.zio"     %% "zio-schema-derivation" % zioSchemaDerivationVersion % Test
    )
  )
  .jsSettings(jsSettings)

// --- core (JS only) --------------------------------------------------------

lazy val core = project
  .in(file("core/js"))
  .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin)
  .dependsOn(model.js, macros)
  .settings(commonSettings)
  .settings(jsSettings)
  .settings(
    name               := "golem-scala-core",
    crossScalaVersions := Seq(Scala3Golem, Scala213),
    libraryDependencies ++= Seq(
      "dev.zio"           %%% "zio-test"                   % zioTestVersion       % Test,
      "dev.zio"           %%% "zio-test-sbt"               % zioTestVersion       % Test,
      "io.github.cquiroz" %%% "scala-java-time"            % scalaJavaTimeVersion % Test,
      "io.github.cquiroz" %%% "scala-java-time-tzdb"       % scalaJavaTimeVersion % Test,
      "io.github.cquiroz" %%% "scala-java-locales"         % "1.5.4"             % Test,
      "io.github.cquiroz" %%% "locales-full-currencies-db" % "1.5.4"             % Test
    )
  )
  .settings(versionSpecificSourceDirs(Compile))
  .settings(versionSpecificSourceDirs(Test))

// --- macros (JVM only) -----------------------------------------------------

lazy val macros = project
  .in(file("macros"))
  .dependsOn(model.jvm)
  .settings(commonSettings)
  .settings(
    name               := "golem-scala-macros",
    crossScalaVersions := Seq(Scala3Golem, Scala213),
    scalacOptions += "-language:experimental.macros",
    libraryDependencies ++= {
      CrossVersion.partialVersion(scalaVersion.value) match {
        case Some((2, _)) =>
          Seq("org.scala-lang" % "scala-reflect" % scalaVersion.value)
        case _ => Nil
      }
    },
    libraryDependencies ++= Seq(
      "dev.zio"     %% "zio-test"              % zioTestVersion             % Test,
      "dev.zio"     %% "zio-test-sbt"          % zioTestVersion             % Test,
      "com.lihaoyi" %% "ujson"                 % ujsonVersion               % Test,
      "dev.zio"     %% "zio-schema-derivation" % zioSchemaDerivationVersion % Test
    )
  )
  .settings(versionSpecificSourceDirs(Compile))
  .settings(versionSpecificSourceDirs(Test))

// --- codegen (JVM only, cross 2.12 + 3.8) ----------------------------------

lazy val codegen = project
  .in(file("codegen"))
  .settings(commonSettings)
  .settings(
    name               := "golem-scala-codegen",
    scalaVersion       := Scala3Golem,
    crossScalaVersions := Seq(Scala212, Scala3Golem),
    libraryDependencies ++= Seq(
      "org.scalameta" %% "scalameta" % scalametaVersion,
      "com.lihaoyi"   %% "ujson"     % ujsonVersion,
      "org.scalameta" %% "munit"     % munitVersion % Test
    )
  )

// --- sbt plugin (2.12 only) ------------------------------------------------

lazy val sbtPlugin = project
  .in(file("sbt"))
  .enablePlugins(SbtPlugin)
  .dependsOn(codegen)
  .settings(
    name               := "golem-scala-sbt",
    scalaVersion       := Scala212,
    crossScalaVersions := Seq(Scala212),
    sbtVersion         := "1.12.0",
    addSbtPlugin("org.scala-js" % "sbt-scalajs" % "1.20.2"),
    libraryDependencies += "org.scalameta" %% "scalafmt-dynamic" % scalafmtDynamicVersion
  )

// --- test-agents (JS, not published) ---------------------------------------

lazy val testAgents = project
  .in(file("test-agents"))
  .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
  .dependsOn(core, macros)
  .settings(commonSettings)
  .settings(jsSettings)
  .settings(
    name               := "golem-scala-test-agents",
    golem.sbt.GolemPlugin.autoImport.golemBasePackage := Some("example"),
    crossScalaVersions := Seq(Scala3Golem, Scala213),
    publish / skip     := true,
    scalaJSUseMainModuleInitializer := false,
    scalaJSLinkerConfig ~= {
      _.withModuleKind(org.scalajs.linker.interface.ModuleKind.ESModule)
    },
    Test / scalaJSLinkerConfig ~= {
      _.withModuleKind(org.scalajs.linker.interface.ModuleKind.CommonJSModule)
    },
    Test / test / skip := true, // requires golem runtime
    libraryDependencies ++= Seq(
      zioBlocksDep("schema").value,
      "io.github.cquiroz" %%% "scala-java-time"      % scalaJavaTimeVersion,
      "io.github.cquiroz" %%% "scala-java-time-tzdb"  % scalaJavaTimeVersion,
      "dev.zio"           %%% "zio-http"              % zioHttpVersion
    ),
    scalacOptions ++= {
      CrossVersion.partialVersion(scalaVersion.value) match {
        case Some((3, _)) => Seq("-Wconf:cat=unused:s")
        case _            => Seq("-Wconf:cat=unused:s")
      }
    }
  )
  .settings(versionSpecificSourceDirs(Compile))
  .settings(versionSpecificSourceDirs(Test))

// --- integration-tests (JVM, not published) --------------------------------

lazy val integrationTests = project
  .in(file("integration-tests"))
  .settings(commonSettings)
  .settings(
    name               := "golem-scala-integration-tests",
    crossScalaVersions := Seq(Scala3Golem),
    publish / skip     := true,
    fork               := true,
    Test / parallelExecution := false,
    Test / envVars ++= sys.env
      .get("GOLEM_TS_PACKAGES_PATH")
      .map(v => Map("GOLEM_TS_PACKAGES_PATH" -> v))
      .getOrElse(Map.empty),
    libraryDependencies ++= Seq(
      "dev.zio" %% "zio-test"     % zioTestVersion    % Test,
      "dev.zio" %% "zio-test-sbt" % zioTestVersion    % Test,
      "dev.zio" %% "zio-process"  % zioProcessVersion % Test
    )
  )

// ---------------------------------------------------------------------------
// Command aliases
// ---------------------------------------------------------------------------

addCommandAlias(
  "golemTest3",
  s"""; ++$Scala3Golem
     ; modelJVM/test
     ; modelJS/test
     ; core/test
     ; macros/test
     ; testAgents/fastLinkJS
     """.stripMargin
)

addCommandAlias(
  "golemTest2",
  s"""; ++$Scala213
     ; modelJVM/test
     ; modelJS/test
     ; core/test
     ; macros/test
     ; testAgents/fastLinkJS
     """.stripMargin
)

addCommandAlias("golemTestAll", "; golemTest3; golemTest2")

addCommandAlias(
  "golemPublishLocal",
  "; set every version := \"0.0.0-SNAPSHOT\"; +publishLocal"
)
