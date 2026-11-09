plugins {
    kotlin("multiplatform") version "2.4.0"
    `maven-publish`
    id("org.jlleitschuh.gradle.ktlint") version "14.2.0"
    id("io.gitlab.arturbosch.detekt") version "1.23.8"
}

group = "cloud.golem"
// 0.1.0 is the release version of the Kotlin SDK artifacts (sdk, ksp, gradle-plugin move together). CI and local
// development publish a snapshot with `-PgolemSdkVersion=0.0.0-SNAPSHOT`.
version = (findProperty("golemSdkVersion") as String?) ?: "0.1.0"

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

// Published to mavenLocal (0.1.0, or the -PgolemSdkVersion snapshot) for local/dev/CI so agent projects resolve
// `cloud.golem:golem-kotlin-sdk` (wasmWasi klib) from mavenLocal.
publishing {
    repositories { mavenLocal() }
}

// Detekt enforces the four rules this project chose: LongMethod (70), ReturnCount,
// TooGenericExceptionCaught and UnsafeCallOnNullableType. The configuration is shared across the
// three modules; each keeps its own baseline, because the findings differ per module.
detekt {
    buildUponDefaultConfig = false
    config.setFrom(rootProject.file("../detekt.yml"))
    parallel = true
}

// One baseline per source set. Detekt's per-source-set tasks all write to the single `baseline`
// property, so leaving it at the default makes each task clobber the previous one's file; naming
// the file after the task keeps them separate. The typed per-source-set tasks are the ones that
// matter: `UnsafeCallOnNullableType` needs type resolution, which the untyped aggregate task does
// not do.
tasks.withType<io.gitlab.arturbosch.detekt.Detekt>().configureEach {
    // Only when it exists: detekt treats the baseline as a required input, and a source set with no
    // findings has no baseline to point at. A finding that appears there later fails the task, which
    // is the ratchet doing its job.
    val recorded = rootProject.file("detekt-baseline-${name.removePrefix("detekt")}.xml")
    if (recorded.exists()) baseline.set(recorded)
}
tasks.withType<io.gitlab.arturbosch.detekt.DetektCreateBaselineTask>().configureEach {
    baseline.set(rootProject.file("detekt-baseline-${name.removePrefix("detektBaseline")}.xml"))
}
