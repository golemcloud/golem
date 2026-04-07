import org.scalajs.linker.interface.ModuleKind

lazy val component_name = project
  .in(file("componentDir"))
  .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
  .settings(
    name := "component-name",
    scalaJSUseMainModuleInitializer := false,
    scalacOptions += "-experimental",
    Compile / scalaJSLinkerConfig ~= (_.withModuleKind(ModuleKind.ESModule)),
    libraryDependencies ++= Seq(
      "cloud.golem" %%% "golem-scala-core"  % "GOLEM_SCALA_SDK_VERSION",
      "cloud.golem" %%% "golem-scala-model" % "GOLEM_SCALA_SDK_VERSION",
      "cloud.golem" %% "golem-scala-macros" % "GOLEM_SCALA_SDK_VERSION"
    ),
    golem.sbt.GolemPlugin.autoImport.golemBasePackage := Some("component_name")
  )
