import org.scalajs.linker.interface.ModuleKind

ThisBuild / scalaVersion := "3.8.2"

def roleProject(projectId: String, directory: String, basePackage: String) =
  Project(projectId, file(directory))
    .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
    .settings(
      name := s"scala-tool-middleware-$directory",
      scalaJSUseMainModuleInitializer := false,
      scalacOptions += "-experimental",
      Compile / scalaJSLinkerConfig ~= (_.withModuleKind(ModuleKind.ESModule)),
      libraryDependencies ++= Seq(
        "cloud.golem" %%% "golem-scala-core"   % "0.0.0-SNAPSHOT",
        "cloud.golem" %%% "golem-scala-model"  % "0.0.0-SNAPSHOT",
        "cloud.golem" %% "golem-scala-macros" % "0.0.0-SNAPSHOT"
      ),
      golem.sbt.GolemPlugin.autoImport.golemBasePackage := Some(basePackage)
    )

lazy val scala_tool_middleware_roles_ordinary =
  roleProject("scala_tool_middleware_roles_ordinary", "ordinary", "roles.ordinary")
lazy val scala_tool_middleware_roles_middleware =
  roleProject("scala_tool_middleware_roles_middleware", "middleware", "roles.middleware")
lazy val scala_tool_middleware_roles_combined =
  roleProject("scala_tool_middleware_roles_combined", "combined", "roles.combined")
