import java.io.ByteArrayOutputStream
import javax.inject.Inject

plugins {
    kotlin("multiplatform") version "2.4.0"
    id("com.google.devtools.ksp") version "2.3.9"
    id("cloud.golem.wasm-component") version "0.0.0-SNAPSHOT"
}

// Must match the version `publishToMavenLocal` was run with.
val golemSdkVersion = (findProperty("golemSdkVersion") as String?) ?: "0.0.0-SNAPSHOT"

// The component size must stay a multiple of 64 KiB pages; this value reaches the generated entry point in bytes.
val headroomKiB = 256

repositories {
    mavenLocal()
    mavenCentral()
}

// Applies the real plugin to a real Kotlin/Wasm + KSP project and runs the whole `nativeComponent` chain with the
// real `wasm-tools`. The agent is the example's, read in place. `witNativeDir` is left unset on purpose, so the
// plugin extracts its bundled `wit-native.zip`.
kotlin {
    @OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)
    wasmWasi {
        binaries.executable()
        nodejs()
    }

    sourceSets {
        val wasmWasiMain by getting {
            kotlin.srcDir("../../example/src/wasmWasiMain/kotlin")
            dependencies {
                implementation("cloud.golem:golem-kotlin-sdk:$golemSdkVersion")
            }
        }
    }
}

dependencies {
    add("kspWasmWasi", "cloud.golem:golem-kotlin-ksp:$golemSdkVersion")
}

wasmComponent {
    moduleName.set("plugin-check")
    arenaHeadroomKiB.set(headroomKiB)
}

abstract class VerifyComponent : DefaultTask() {
    @get:InputFile
    abstract val component: RegularFileProperty

    @get:InputDirectory
    abstract val generatedSources: DirectoryProperty

    @get:Input
    abstract val wasmTools: Property<String>

    @get:Input
    abstract val headroomBytes: Property<Int>

    @get:Inject
    abstract val exec: ExecOperations

    @TaskAction
    fun verify() {
        val wit = ByteArrayOutputStream()
        exec.exec {
            commandLine(wasmTools.get(), "component", "wit", component.get().asFile.absolutePath)
            standardOutput = wit
        }
        val text = wit.toString()
        check("golem:agent/guest" in text) { "the component does not export golem:agent/guest:\n$text" }

        val expected = "configureArenaHeadroom(${headroomBytes.get()})"
        val registrations = generatedSources.get().asFile.walkTopDown().filter { it.isFile && it.extension == "kt" }.toList()
        check(registrations.any { expected in it.readText() }) {
            "no generated source contains `$expected`: wasmComponent.arenaHeadroomKiB did not reach the KSP processor"
        }
    }
}

tasks.register<VerifyComponent>("verifyComponent") {
    group = "verification"
    description = "Checks the component that nativeComponent built and the option the plugin forwarded to KSP."
    dependsOn("nativeComponent")
    component.set(wasmComponent.outputWasm)
    generatedSources.set(layout.buildDirectory.dir("generated/ksp"))
    wasmTools.set(wasmComponent.wasmTools)
    headroomBytes.set(headroomKiB * 1024)
}

// `wasmComponent.wasiAdapterPath` replaces the downloaded adapter. Each task below is a second `nativeComponent` that
// shares the first one's compiled core module and differs only in its adapter and output.
val adapterOverrideDir = layout.buildDirectory.dir("adapter-override")

fun overrideTask(name: String, adapter: Provider<String>) = tasks.register<cloud.golem.gradle.NativeComponentTask>(name) {
    val base = tasks.named<cloud.golem.gradle.NativeComponentTask>("nativeComponent")
    coreWasmDir.set(base.flatMap { it.coreWasmDir })
    worldName.set(base.flatMap { it.worldName })
    wasmTools.set(base.flatMap { it.wasmTools })
    wasiAdapterPath.set(adapter)
    outputWasm.set(adapterOverrideDir.map { it.file("$name.wasm") })
}

val validOverride = overrideTask("adapterOverrideValid", adapterOverrideDir.map { it.file("custom-adapter.wasm").asFile.path })
val garbageOverride = overrideTask("adapterOverrideGarbage", adapterOverrideDir.map { it.file("garbage-adapter.wasm").asFile.path })
val missingOverride = overrideTask("adapterOverrideMissing", adapterOverrideDir.map { it.file("no-such-adapter.wasm").asFile.path })

tasks.register("verifyAdapterOverride") {
    group = "verification"
    description = "Checks that wasmComponent.wasiAdapterPath is the adapter wasm-tools is given, and is never replaced by a download."
    // The default run downloads and caches the pinned adapter, which supplies a known-good file to copy.
    dependsOn("nativeComponent")
    val defaultComponent = wasmComponent.outputWasm
    doLast {
        val dir = adapterOverrideDir.get().asFile.apply { deleteRecursively(); mkdirs() }

        // A valid adapter at a path the plugin does not manage gives the same component as the default run.
        val gradleHome = System.getenv("GRADLE_USER_HOME")?.takeIf { it.isNotBlank() } ?: "${System.getProperty("user.home")}/.gradle"
        val cached = File(gradleHome, "caches/golem/wasi-adapter").walkTopDown().firstOrNull { it.name == "wasi_snapshot_preview1.reactor.wasm" }
            ?: error("the default run left no adapter under $gradleHome/caches/golem/wasi-adapter")
        cached.copyTo(File(dir, "custom-adapter.wasm"))
        validOverride.get().componentize()
        check(File(dir, "adapterOverrideValid.wasm").readBytes().contentEquals(defaultComponent.get().asFile.readBytes())) {
            "a component built with an explicit copy of the adapter differs from the default component"
        }

        // A file that is not an adapter must reach wasm-tools and fail there. A download would have succeeded.
        File(dir, "garbage-adapter.wasm").writeText("not a wasm module")
        val garbageFailure = runCatching { garbageOverride.get().componentize() }.exceptionOrNull()
        checkNotNull(garbageFailure) { "wasiAdapterPath pointing at a file that is not an adapter did not fail: the override was ignored" }
        check(!File(dir, "adapterOverrideGarbage.wasm").exists()) { "a component was produced from a file that is not an adapter" }

        // A missing file is refused by name, not replaced by a download.
        val missingFailure = runCatching { missingOverride.get().componentize() }.exceptionOrNull()
        check(missingFailure?.message?.contains("wasmComponent.wasiAdapterPath does not exist") == true) {
            "a missing wasiAdapterPath was not refused by name: ${missingFailure?.message}"
        }
    }
}
