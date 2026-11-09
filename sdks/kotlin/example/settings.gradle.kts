rootProject.name = "counter-agent"

// SDK, KSP processor, and the wasm-component Gradle plugin are resolved as published
// artifacts from mavenLocal (cloud.golem:* at 0.1.0). Publish them first:
//   (cd ../sdk && ./gradlew publishToMavenLocal)
//   (cd ../ksp && ./gradlew publishToMavenLocal)
//   (cd ../gradle-plugin && ./gradlew publishToMavenLocal)
pluginManagement {
    repositories {
        mavenLocal()
        gradlePluginPortal()
    }
}
