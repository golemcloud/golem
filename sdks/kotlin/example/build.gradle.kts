plugins {
    kotlin("multiplatform") version "2.4.0"
    id("com.google.devtools.ksp") version "2.3.9"
    id("cloud.golem.wasm-component") version "0.0.0-SNAPSHOT"
}

group = "cloud.golem.example"
version = "0.1.0-SNAPSHOT"

repositories {
    mavenLocal()
    mavenCentral()
}

kotlin {
    js {
        outputModuleName.set("counter-agent")
        binaries.executable()
        nodejs()
        useEsModules()
    }

    sourceSets {
        val jsMain by getting {
            dependencies {
                // The SDK is a NORMAL Kotlin/JS dependency — compiled + bundled into the agent
                // JS bundle. BaseAgent + annotations + runtime all come from here.
                implementation("cloud.golem:golem-kotlin-sdk:0.0.0-SNAPSHOT")
            }
        }
    }
}

dependencies {
    // Compile-time-only annotation processor: generates <Class>Registration.kt +
    // GolemGeneratedMain.kt (owns main()) + <component>-agent.wit.
    add("kspJs", "cloud.golem:golem-kotlin-ksp:0.0.0-SNAPSHOT")
}

// `golem build` runs `bundleAgentJs` (produces the single agent JS bundle that exports `guest`),
// then injects it into the prebuilt agent_guest.wasm via the manifest's injectToPrebuiltQuickjs.
// Only the Golem host interface is external (provided by the wasm host at runtime).
wasmComponent {
    moduleName.set("counter-agent")
    externals.add("golem:api/host@1.5.0")
}
