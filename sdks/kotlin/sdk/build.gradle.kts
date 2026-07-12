plugins {
    kotlin("multiplatform") version "2.4.0"
    `maven-publish`
    id("org.jlleitschuh.gradle.ktlint") version "14.2.0"
}

group = "cloud.golem"
version = "0.0.0-SNAPSHOT"

repositories {
    mavenCentral()
}

kotlin {
    // Native path: compile Kotlin/Wasm (WasmGC) directly to a Wasm Component (no JS/QuickJS).
    // The agent compiles + links the SDK, which is componentized via wasm-tools.
    @OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)
    wasmWasi {
        nodejs()
        binaries.executable()
    }

    sourceSets {
        val commonMain by getting
        val wasmWasiMain by getting
        val wasmWasiTest by getting {
            dependencies {
                implementation(kotlin("test"))
            }
        }
    }
}

// Published to mavenLocal at 0.0.0-SNAPSHOT for local/dev/CI so agent projects resolve
// `cloud.golem:golem-kotlin-sdk` (wasmWasi klib) from mavenLocal.
publishing {
    repositories { mavenLocal() }
}
