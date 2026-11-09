plugins {
    kotlin("multiplatform")
    id("com.google.devtools.ksp")
    id("cloud.golem.wasm-component")
}

group = "cloud.golem.app"
version = "0.1.0-SNAPSHOT"

repositories {
    mavenLocal()
    mavenCentral()
}

val golemKotlinVersion = providers.gradleProperty("golemKotlinVersion").get()

kotlin {
    @OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)
    wasmWasi {
        binaries.executable()
        nodejs()
    }

    sourceSets {
        val wasmWasiMain by getting {
            dependencies {
                implementation("cloud.golem:golem-kotlin-sdk:$golemKotlinVersion")
            }
        }
    }
}

dependencies {
    add("kspWasmWasi", "cloud.golem:golem-kotlin-ksp:$golemKotlinVersion")
}

wasmComponent {
    moduleName.set("kotlin_examples")
}
