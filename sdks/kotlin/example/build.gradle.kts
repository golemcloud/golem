plugins {
    kotlin("multiplatform") version "2.4.0"
    id("com.google.devtools.ksp") version "2.3.9"
    id("cloud.golem.wasm-component") version "0.0.0-SNAPSHOT"
    id("org.jlleitschuh.gradle.ktlint") version "14.2.0"
}

group = "cloud.golem.example"
version = "0.1.0-SNAPSHOT"

repositories {
    mavenLocal()
    mavenCentral()
}

kotlin {
    @OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)
    wasmWasi {
        binaries.executable()
        nodejs()
    }

    sourceSets {
        val wasmWasiMain by getting {
            dependencies {
                // The SDK is a NORMAL Kotlin/Wasm dependency, compiled directly into this
                // agent's own wasm module (no JS bundling, no QuickJS).
                implementation("cloud.golem:golem-kotlin-sdk:0.0.0-SNAPSHOT")
            }
        }
    }
}

dependencies {
    // Compile-time-only annotation processor: generates <Class>Registration.kt +
    // GolemGeneratedGuest.kt (the real @WasmExport golem:agent/guest@2.0.0 functions).
    add("kspWasmWasi", "cloud.golem:golem-kotlin-ksp:0.0.0-SNAPSHOT")
}

wasmComponent {
    moduleName.set("counter-agent")
    witNativeDir.set(file("../wit-native"))
}

tasks.withType<org.jlleitschuh.gradle.ktlint.tasks.BaseKtLintCheckTask>().configureEach {
    exclude { it.file.path.contains("/build/generated/") }
}
