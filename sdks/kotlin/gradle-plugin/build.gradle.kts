plugins {
    kotlin("jvm") version "2.4.0"
    `java-gradle-plugin`
    `maven-publish`
}

group = "cloud.golem"
version = "0.0.0-SNAPSHOT"

repositories {
    mavenCentral()
}

gradlePlugin {
    plugins {
        create("wasmComponent") {
            id = "cloud.golem.wasm-component"
            implementationClass = "cloud.golem.gradle.WasmComponentPlugin"
            displayName = "Golem Kotlin wasm-component plugin"
            description = "Builds a Golem Wasm Component from a Kotlin/JS agent: " +
                "KSP -> Kotlin/JS IR -> rollup -> wasm-rquickjs inject-js -> wasm-tools validate."
        }
    }
}
