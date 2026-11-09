plugins {
    kotlin("jvm") version "2.4.0"
    `java-gradle-plugin`
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

dependencies {
    // ProjectBuilder (org.gradle.testfixtures) comes from gradleApi(), which java-gradle-plugin
    // already puts on the classpath; these add the JUnit Platform test runtime + kotlin.test.
    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.10.2")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.test {
    useJUnitPlatform()
}

gradlePlugin {
    plugins {
        create("wasmComponent") {
            id = "cloud.golem.wasm-component"
            implementationClass = "cloud.golem.gradle.WasmComponentPlugin"
            displayName = "Golem Kotlin native wasm-component plugin"
            description = "Builds a Golem Wasm Component directly from Kotlin/Wasm (WasmGC): " +
                "KSP -> Kotlin/Wasm (wasmWasi) -> wasm-tools component embed -> " +
                "wasm-tools component new (WASI p1->p2 adapter). No JS, no QuickJS."
        }
    }
}

publishing {
    repositories { mavenLocal() }
}

// Bundle the canonical sdks/kotlin/wit-native/ (single source of truth, also used directly by
// sdks/kotlin/example/) as a single resource inside the plugin jar, so a `golem new`-scaffolded
// project anywhere on disk doesn't need its own copy: NativeComponentTask extracts this at build
// time when the consuming project doesn't set wasmComponent.witNativeDir explicitly.
val witNativeSrc = layout.projectDirectory.dir("../wit-native")
val witNativeResourcesDir = layout.buildDirectory.dir("generated-resources")

val zipWitNative by tasks.registering(Zip::class) {
    from(witNativeSrc)
    archiveFileName.set("wit-native.zip")
    destinationDirectory.set(witNativeResourcesDir)
    includeEmptyDirs = false
}

sourceSets {
    main {
        resources.srcDir(witNativeResourcesDir)
    }
}

tasks.processResources {
    dependsOn(zipWitNative)
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
