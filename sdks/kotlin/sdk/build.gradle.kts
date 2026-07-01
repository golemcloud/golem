plugins {
    kotlin("multiplatform") version "2.4.0"
    `maven-publish`
}

group = "cloud.golem"
version = "0.0.0-SNAPSHOT"

repositories {
    mavenCentral()
}

kotlin {
    js {
        outputModuleName.set("golem-kotlin-sdk")
        // A LIBRARY (klib) consumed by agent projects — the agent compiles + bundles the SDK
        // into its own JS bundle (Scala model). NOT an executable.
        binaries.library()
        nodejs()
        useEsModules()
    }

    sourceSets {
        val commonMain by getting
        val jsMain by getting
        val jsTest by getting {
            dependencies {
                implementation(kotlin("test"))
            }
        }
    }
}

// Published to mavenLocal at 0.0.0-SNAPSHOT for local/dev/CI (the Scala `golemPublishLocal`
// analogue); agent projects resolve `cloud.golem:golem-kotlin-sdk` from mavenLocal.
publishing {
    repositories { mavenLocal() }
}
