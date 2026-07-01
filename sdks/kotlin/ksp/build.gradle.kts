plugins {
    kotlin("jvm") version "2.4.0"
    `maven-publish`
}

group = "cloud.golem"
version = "0.0.0-SNAPSHOT"

repositories {
    mavenCentral()
}

dependencies {
    // KSP2 API (standalone versioning, decoupled from the Kotlin patch; works with 2.4.0).
    // The processor runs on the JVM, not Wasm.
    implementation("com.google.devtools.ksp:symbol-processing-api:2.3.9")

    // KotlinPoet for code generation.
    implementation("com.squareup:kotlinpoet:1.18.1")

    testImplementation(kotlin("test"))
    // Maintained kotlin-compile-testing fork (supports KSP2 + recent Kotlin).
    testImplementation("dev.zacsweers.kctfork:ksp:0.13.0")
}

tasks.test {
    useJUnitPlatform()
}

// kctfork's compile-testing API is annotated @ExperimentalCompilerApi.
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    compilerOptions {
        optIn.add("org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi")
    }
}

publishing {
    repositories { mavenLocal() }
    publications {
        create<MavenPublication>("maven") { from(components["java"]) }
    }
}
