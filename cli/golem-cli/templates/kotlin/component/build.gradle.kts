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
    js {
        // The Kotlin/JS module name. It must match `wasmComponent.moduleName` below and the
        // `{{ component_name | to_snake_case }}` the build template injects, so the plugin and
        // `golem build` agree on the produced bundle path (build/golem/<name>.js).
        outputModuleName.set("component_name")
        binaries.executable()
        nodejs()
        useEsModules()
    }

    sourceSets {
        val jsMain by getting {
            dependencies {
                // The Golem Kotlin SDK is a NORMAL Kotlin/JS dependency — it is compiled and
                // bundled into this agent's JS bundle. Resolves from mavenLocal.
                implementation("cloud.golem:golem-kotlin-sdk:0.0.0-SNAPSHOT")
            }
        }
    }
}

dependencies {
    // KSP processor: reads the @Agent classes and generates the registration glue + main().
    add("kspJs", "cloud.golem:golem-kotlin-ksp:0.0.0-SNAPSHOT")
}

// `golem build` runs the plugin's `bundleAgentJs` task (single ESM bundle that exports `guest`,
// SDK included), then injects it into the prebuilt agent_guest.wasm. Only the Golem host
// interface is external (provided by the Wasm host at runtime).
wasmComponent {
    moduleName.set("component_name")
    externals.add("golem:api/host@1.5.0")
}
