plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

android {
    namespace = "live.renoa.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "live.renoa.app"
        minSdk = 28
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        // The Renoa the app signs in to; its passkeys and assetlinks.json belong to this origin.
        buildConfigField("String", "RENOA_ORIGIN", "\"https://renoa.live\"")
        buildConfigField("String", "RENOA_CONNECT", "\"wss://renoa.live/connect\"")
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
        unitTests.isIncludeAndroidResources = true
        // Robolectric reaches into the JDK's file-descriptor internals.
        unitTests.all {
            it.jvmArgs("--add-opens=java.base/jdk.internal.access=ALL-UNNAMED")
            // The Host's published schemas, which the app's decoders are checked against.
            it.systemProperty("renoa.docs", rootProject.file("../../docs").absolutePath)
        }
    }

    lint {
        warningsAsErrors = true
        abortOnError = true
        // The platform is pinned to 36 on purpose, the level the build and CI images carry.
        // Moving to a newer one is a decision, not something lint should demand on release day.
        disable += setOf("OldTargetApi", "GradleDependency", "NewerVersionAvailable")
    }
}

kotlin {
    jvmToolchain(21)
    compilerOptions {
        allWarningsAsErrors.set(true)
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.ui.text)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.kotlinx.coroutines.android)
    implementation(project(":rcp"))
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.androidx.credentials)
    // Credential Manager reaches passkeys through Play services on Android 13 and earlier.
    implementation(libs.androidx.credentials.play.services)

    testImplementation(libs.junit)
    testImplementation(libs.kotlin.test)
    testImplementation(libs.kotlin.reflect)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.compose.ui.test.junit4)
    debugImplementation(libs.androidx.compose.ui.test.manifest)
    testImplementation(libs.kotlinx.coroutines.test)
}
