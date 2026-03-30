import org.scalajs.linker.interface.ModuleKind

ThisBuild / scalaVersion := "GOLEM_SCALA_VERSION"

lazy val root = project
  .in(file("."))
  .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
  .settings(
    name := "app-name",
    scalaJSUseMainModuleInitializer := false,
    scalacOptions += "-experimental",
    Compile / scalaJSLinkerConfig ~= (_.withModuleKind(ModuleKind.ESModule)),
    libraryDependencies ++= Seq(
      "cloud.golem" %%% "golem-scala-core"  % "GOLEM_SCALA_SDK_VERSION",
      "cloud.golem" %%% "golem-scala-model" % "GOLEM_SCALA_SDK_VERSION",
      "cloud.golem" %% "golem-scala-macros" % "GOLEM_SCALA_SDK_VERSION"
    ),
    golemBasePackage := Some("component_name")
  )
