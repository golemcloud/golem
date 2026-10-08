import org.scalajs.linker.interface.ModuleKind

ThisBuild / scalaVersion := "3.8.2"

lazy val scala_native_tool_source_consumer =
  Project("scala_native_tool_source_consumer", file("."))
    .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
    .settings(
      name := "scala-native-tool-source",
      scalaJSUseMainModuleInitializer := false,
      scalacOptions += "-experimental",
      Compile / scalaJSLinkerConfig ~= (_.withModuleKind(ModuleKind.ESModule)),
      Compile / unmanagedSourceDirectories +=
        (LocalRootProject / baseDirectory).value / "golem-temp/bridge-sdk/scala/internal/native-conformance-tool-guest-client/src/main/scala",
      libraryDependencies ++= Seq(
        "cloud.golem" %%% "golem-scala-core"   % "0.0.0-SNAPSHOT",
        "cloud.golem" %%% "golem-scala-model"  % "0.0.0-SNAPSHOT",
        "cloud.golem" %% "golem-scala-macros" % "0.0.0-SNAPSHOT"
      ),
      golem.sbt.GolemPlugin.autoImport.golemBasePackage := Some("sources.scala.native")
    )
