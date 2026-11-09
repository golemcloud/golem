// The Golem Kotlin SDK, its KSP processor, and the `cloud.golem.wasm-component` Gradle plugin
// are resolved as published artifacts from mavenLocal (cloud.golem:* at the version pinned below).
// Publish them once before the first `golem build`:
//   (cd <golem>/sdks/kotlin/sdk           && ./gradlew publishToMavenLocal)
//   (cd <golem>/sdks/kotlin/ksp           && ./gradlew publishToMavenLocal)
//   (cd <golem>/sdks/kotlin/gradle-plugin && ./gradlew publishToMavenLocal)
pluginManagement {
    repositories {
        mavenLocal()
        gradlePluginPortal()
    }
    // Plugin versions are pinned here once, so component build files carry no versions.
    plugins {
        kotlin("multiplatform") version "2.4.0"
        id("com.google.devtools.ksp") version "2.3.9"
        id("cloud.golem.wasm-component") version "GOLEM_KOTLIN_SDK_VERSION"
    }
}

// Every directory under the app root that holds a build.gradle.kts is a component project. With one component the
// component lives at the app root and there is nothing to include; after `golem new` adds a second component each
// component moves into its own directory and is picked up here without editing this file.
rootDir.listFiles { f -> f.isDirectory && !f.name.startsWith(".") && File(f, "build.gradle.kts").exists() }
    ?.sortedBy { it.name }
    ?.forEach { include(it.name) }
