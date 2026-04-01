import org.scalajs.linker.interface.ModuleKind

ThisBuild / scalaVersion := "3.8.2"

lazy val root = project
  .in(file("."))
  .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
  .settings(
    name := "scala-demo",
    scalaJSUseMainModuleInitializer := false,
    scalacOptions += "-experimental",
    Compile / scalaJSLinkerConfig ~= (_.withModuleKind(ModuleKind.ESModule)),
    golemAgentGuestWasmFile := {
      val appRoot = (ThisProject / baseDirectory).value
      appRoot / ".generated" / "agent_guest.wasm"
    },
    libraryDependencies ++= Seq(
      "dev.zio" %%% "zio-golem-core"  % "0.0.0-SNAPSHOT",
      "dev.zio" %%% "zio-golem-model" % "0.0.0-SNAPSHOT",
      "dev.zio" %% "zio-golem-macros" % "0.0.0-SNAPSHOT"
    ),
    golemBasePackage := Some("demo")
  )

