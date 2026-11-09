rootProject.name = "real-sdk-compile"

// The SDK and the KSP processor resolve as published artifacts from mavenLocal. Publish them first:
//   for m in sdk ksp; do (cd ../../$m && ./gradlew publishToMavenLocal -PgolemSdkVersion=0.0.0-SNAPSHOT); done
pluginManagement {
    repositories {
        mavenLocal()
        gradlePluginPortal()
    }
}

// One module per fixture directory. The processor writes a single native entry point per module, and it
// requires every `@Agent` class of the module to share a package, so fixtures that use different packages
// cannot share a compilation. A directory whose files use different packages gets one module per file.
// The fixtures are source inputs of their module and are not copied.
val fixtures = mapOf(
    "config-agents" to "../config-agents",
    "contract-probe" to "../contract-tests/ContractProbeAgent.kt",
    "headroom-probe" to "../contract-tests/HeadroomProbeAgent.kt",
    "durable-streams" to "../durable-streams",
    "http-bindings" to "../http-bindings",
    "router-agents" to "../http-routers/RouterAgents.kt",
    "router-handler-agents" to "../http-routers/RouterHandlerAgents.kt",
    "permission-cards" to "../permission-cards",
    "reflection" to "../reflection",
    "restricted-agents" to "../schema-corners/RestrictedAgents.kt",
    "schema-corners-agents" to "../schema-corners/SchemaCornersAgents.kt",
    "snapshots" to "../snapshots",
    "tool-middleware" to "../tool-middleware",
    "tool-reflection" to "../tool-reflection",
    "websocket" to "../websocket",
)

gradle.extra["fixtureDirs"] = fixtures

fixtures.keys.forEach { name ->
    include(name)
    project(":$name").projectDir = file("modules/$name")
}
