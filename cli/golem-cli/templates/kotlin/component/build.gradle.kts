plugins {
    kotlin("multiplatform") version "2.4.0"
    id("com.google.devtools.ksp") version "2.3.9"
    id("cloud.golem.wasm-component") version "0.0.0-SNAPSHOT"
}

group = "cloud.golem.app"
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
                // The Golem Kotlin SDK is a NORMAL Kotlin/Wasm dependency -- it is compiled
                // directly into this agent's own wasm module. No JS, no bundling. Resolves from
                // mavenLocal.
                implementation("cloud.golem:golem-kotlin-sdk:0.0.0-SNAPSHOT")
            }
        }
    }
}

dependencies {
    // KSP processor: reads the @Agent classes and generates the real
    // @WasmExport("golem:agent/guest@2.0.0#...") functions + registration.
    add("kspWasmWasi", "cloud.golem:golem-kotlin-ksp:0.0.0-SNAPSHOT")
}

// `golem build` runs the plugin's `nativeComponent` task: Kotlin/Wasm compile -> wasm-tools
// component embed -> wasm-tools component new --adapt (WASI p1->p2) -> wasm-tools validate.
// No JS/QuickJS anywhere in this path.
wasmComponent {
    moduleName.set("component_name")
}
