plugins {
    kotlin("jvm") version "2.4.0"
    `java-gradle-plugin`
    `maven-publish`
    id("org.jlleitschuh.gradle.ktlint") version "14.2.0"
}

group = "cloud.golem"
version = "0.0.0-SNAPSHOT"

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
