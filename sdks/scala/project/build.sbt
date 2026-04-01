lazy val root = (project in file("."))
  .settings(
    Compile / unmanagedSources ++= {
      val repoRoot   = baseDirectory.value.getParentFile // sdks/scala
      val codegenDir = repoRoot / "codegen" / "src" / "main" / "scala" / "golem" / "codegen"
      Seq(
        repoRoot / "sbt" / "src" / "main" / "scala" / "golem" / "sbt" / "GolemPlugin.scala",
        codegenDir / "autoregister" / "AutoRegisterCodegen.scala",
        codegenDir / "discovery" / "SourceDiscovery.scala",
        codegenDir / "ir" / "AgentSurfaceIR.scala",
        codegenDir / "ir" / "AgentSurfaceIRCodec.scala",
        codegenDir / "rpc" / "RpcCodegen.scala",
        codegenDir / "pipeline" / "CodegenPipeline.scala"
      )
    },
    Compile / unmanagedResourceDirectories += {
      val repoRoot = baseDirectory.value.getParentFile
      repoRoot / "sbt" / "src" / "main" / "resources"
    },
    libraryDependencies ++= Seq(
      "org.scalameta" %% "scalameta"        % "4.14.7",
      "org.scalameta" %% "scalafmt-dynamic" % "3.10.4",
      "com.lihaoyi"   %% "ujson"            % "3.1.0"
    )
  )
