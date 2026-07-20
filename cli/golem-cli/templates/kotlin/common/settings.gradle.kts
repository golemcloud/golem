// The Golem Kotlin SDK, its KSP processor, and the `cloud.golem.wasm-component` Gradle plugin
// are resolved as published artifacts from mavenLocal (cloud.golem:* at 0.0.0-SNAPSHOT).
// Publish them once before the first `golem build`:
//   (cd <golem>/sdks/kotlin/sdk           && ./gradlew publishToMavenLocal)
//   (cd <golem>/sdks/kotlin/ksp           && ./gradlew publishToMavenLocal)
//   (cd <golem>/sdks/kotlin/gradle-plugin && ./gradlew publishToMavenLocal)
pluginManagement {
    repositories {
        mavenLocal()
        gradlePluginPortal()
    }
}
