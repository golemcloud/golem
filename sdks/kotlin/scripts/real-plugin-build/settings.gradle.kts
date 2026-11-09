rootProject.name = "real-plugin-build"

// The SDK, the KSP processor and the Gradle plugin resolve as published artifacts from mavenLocal. Publish them first:
//   for m in sdk ksp gradle-plugin; do (cd ../../$m && ./gradlew publishToMavenLocal -PgolemSdkVersion=0.0.0-SNAPSHOT); done
pluginManagement {
    repositories {
        mavenLocal()
        gradlePluginPortal()
    }
}
