import org.jetbrains.kotlin.gradle.dsl.KotlinMultiplatformExtension

plugins {
    kotlin("multiplatform") version "2.4.0" apply false
    id("com.google.devtools.ksp") version "2.3.9" apply false
}

// Must match the version `publishToMavenLocal` was run with.
val golemSdkVersion = (findProperty("golemSdkVersion") as String?) ?: "0.0.0-SNAPSHOT"

// Compiles each acceptance fixture for Kotlin/Wasm, with the KSP processor running over it, against the
// real published SDK. Nothing is componentized or run, so `wasm-tools` and a Golem server are not needed.
@Suppress("UNCHECKED_CAST")
val fixtureDirs = gradle.extra["fixtureDirs"] as Map<String, String>

subprojects {
    val fixtureName = name
    apply(plugin = "org.jetbrains.kotlin.multiplatform")
    apply(plugin = "com.google.devtools.ksp")

    repositories {
        mavenLocal()
        mavenCentral()
    }

    extensions.configure<KotlinMultiplatformExtension> {
        @OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)
        wasmWasi {
            binaries.executable()
            nodejs()
        }

        sourceSets.getByName("wasmWasiMain") {
            val fixture = rootProject.file(fixtureDirs.getValue(fixtureName))
            if (fixture.isDirectory) {
                kotlin.srcDir(fixture)
            } else {
                // KSP reads whole source roots and ignores include filters, so a single file is staged
                // into a directory of its own.
                val stage = tasks.register<Sync>("stageFixture") {
                    from(fixture)
                    into(layout.buildDirectory.dir("fixture"))
                }
                kotlin.srcDir(stage)
            }
            dependencies {
                implementation("cloud.golem:golem-kotlin-sdk:$golemSdkVersion")
            }
        }
    }

    dependencies {
        add("kspWasmWasi", "cloud.golem:golem-kotlin-ksp:$golemSdkVersion")
    }
}
