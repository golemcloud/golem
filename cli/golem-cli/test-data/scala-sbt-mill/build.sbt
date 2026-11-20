import org.scalajs.linker.interface.ModuleKind

ThisBuild / scalaVersion := "3.8.2"

lazy val app = Project("app", file("."))
  .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
  .settings(
    scalaJSUseMainModuleInitializer := false,
    scalacOptions += "-experimental",
    Compile / scalaJSLinkerConfig ~= (_.withModuleKind(ModuleKind.ESModule)),
    libraryDependencies ++= Seq(
      "cloud.golem" %%% "golem-scala-core"   % "0.0.0-SNAPSHOT",
      "cloud.golem" %%% "golem-scala-model"  % "0.0.0-SNAPSHOT",
      "cloud.golem" %%  "golem-scala-macros" % "0.0.0-SNAPSHOT"
    ),
    golem.sbt.GolemPlugin.autoImport.golemBasePackage := Some("buildtools")
  )
