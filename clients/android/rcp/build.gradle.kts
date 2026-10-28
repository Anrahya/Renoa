plugins {
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.kotlin.serialization)
}

kotlin {
    jvmToolchain(21)
    compilerOptions {
        allWarningsAsErrors.set(true)
    }
}

dependencies {
    api(libs.kotlinx.coroutines.core)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.okhttp)

    testImplementation(libs.junit)
    testImplementation(libs.kotlin.test)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.okhttp.mockwebserver)
}

tasks.test {
    useJUnit()
    // The wire tests read the canonical binding document next to the repository root.
    systemProperty("renoa.docs", rootDir.resolve("../../docs").canonicalPath)
}

// One command tests the whole build: `./gradlew testDebugUnitTest` runs the app's debug unit
// tests, and this makes it run this module's tests too.
tasks.register("testDebugUnitTest") {
    dependsOn(tasks.test)
}
