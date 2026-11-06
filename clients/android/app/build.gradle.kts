plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

/**
 * Firebase's client configuration, for push wake-ups. It belongs to the owner's Firebase project and
 * never enters the repository: the build reads it from outside the checkout, from the Gradle property
 * `renoa.googleServices`, else the environment variable `RENOA_GOOGLE_SERVICES`, else
 * `~/.config/renoa/google-services.json`. Without the file the app builds and runs with push off.
 */
val googleServicesNamed: String? =
    providers.gradleProperty("renoa.googleServices").orNull ?: providers.environmentVariable("RENOA_GOOGLE_SERVICES").orNull
val googleServices: File = googleServicesNamed?.let { File(it) } ?: File(System.getProperty("user.home"), ".config/renoa/google-services.json")
if (googleServicesNamed != null && !googleServices.isFile) {
    logger.warn("Renoa: $googleServices, named for google-services.json, is not a file; building with push off.")
}

/**
 * The release this build is, from the Gradle property `renoa.version` (`0.15.2`, a release tag without
 * its `v`). Its version code orders releases, so the app knows a newer one: MAJOR * 1,000,000 +
 * MINOR * 1,000 + PATCH. A build without it is a development build, version code 1.
 */
val renoaVersion: String? = providers.gradleProperty("renoa.version").orNull
val renoaVersionCode: Int =
    renoaVersion?.let { version ->
        val parts = Regex("""(\d{1,3})\.(\d{1,3})\.(\d{1,3})""").matchEntire(version)?.groupValues?.drop(1)?.map(String::toInt)
            ?: error("renoa.version is MAJOR.MINOR.PATCH, each below 1000, not $version")
        parts[0] * 1_000_000 + parts[1] * 1_000 + parts[2]
    } ?: 1

android {
    namespace = "live.renoa.app"
    compileSdk = 37

    defaultConfig {
        applicationId = "live.renoa.app"
        minSdk = 28
        targetSdk = 36
        versionCode = renoaVersionCode
        versionName = renoaVersion ?: "0.0.0-dev"
        // The Renoa the app signs in to; its passkeys and assetlinks.json belong to this origin.
        buildConfigField("String", "RENOA_ORIGIN", "\"https://renoa.live\"")
        buildConfigField("String", "RENOA_CONNECT", "\"wss://renoa.live/connect\"")
        // The newest app Renoa serves, as renoa-deploy places it; see scripts/publish-android-app.
        buildConfigField("String", "RENOA_APP_RELEASE", "\"https://renoa.live/app/android.json\"")
        // Firebase reads these resources as the app starts, as it would those the google-services plugin writes.
        firebaseResources(googleServices, "live.renoa.app").forEach { (name, value) -> resValue("string", name, value) }
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

    // The licences of the fonts, icons and diagram renderer the app bundles travel inside it, as their terms ask.
    sourceSets.getByName("main").assets.directories.add(rootProject.file("licenses").path)

    buildFeatures {
        compose = true
        buildConfig = true
        resValues = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
        unitTests.isIncludeAndroidResources = true
        // Robolectric reaches into the JDK's file-descriptor internals.
        unitTests.all {
            it.jvmArgs("--add-opens=java.base/jdk.internal.access=ALL-UNNAMED")
            // The Host's published schemas, which the app's decoders are checked against.
            it.systemProperty("renoa.docs", rootProject.file("../../docs").absolutePath)
            // The design system's tokens, which the app's colours are checked against.
            it.systemProperty("renoa.design", rootProject.file("../../design").absolutePath)
        }
    }

    lint {
        warningsAsErrors = true
        abortOnError = true
        // The app compiles against 37 but still targets 36, and its pinned AndroidX and OkHttp
        // versions trail the newest releases. Moving either is a decision, not something lint
        // should demand on release day.
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
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.kotlinx.coroutines.android)
    implementation(project(":rcp"))
    implementation(project(":design"))
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.okhttp)
    implementation(libs.androidx.credentials)
    // Credential Manager reaches passkeys through Play services on Android 13 and earlier.
    implementation(libs.androidx.credentials.play.services)
    // Push wake-ups: Firebase Cloud Messaging delivers them, and WorkManager runs the catch-up each one starts.
    implementation(libs.firebase.messaging)
    implementation(libs.androidx.work.runtime)
    // Agent replies render as markdown. Only the core and Material 3 modules: no remote images.
    implementation(libs.markdown.renderer.m3)

    testImplementation(libs.junit)
    testImplementation(libs.kotlin.test)
    testImplementation(libs.kotlin.reflect)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.compose.ui.test.junit4)
    debugImplementation(libs.androidx.compose.ui.tooling)
    debugImplementation(libs.androidx.compose.ui.test.manifest)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(testFixtures(project(":rcp")))
    testImplementation(libs.okhttp.mockwebserver)
}

/**
 * The string resources Firebase initialises itself from, named as the google-services plugin names
 * them, taken from [file]'s entry for [packageName]; none when there is no file. Only the four values
 * Firebase Cloud Messaging needs are read. Errors name the file, never its contents.
 */
fun firebaseResources(file: File, packageName: String): Map<String, String> {
    if (!file.isFile) return emptyMap()
    fun Any?.field(name: String): Any? = (this as? Map<*, *>)?.get(name)
    // The parser's own message quotes the offending text, which may hold the API key, so it is not passed on.
    val json =
        try {
            groovy.json.JsonSlurper().parse(file)
        } catch (_: Exception) {
            throw GradleException("$file is not valid JSON; download it again from the Firebase console")
        }
    val project = json.field("project_info")
    val client =
        (json.field("client") as? List<*>)?.firstOrNull { it.field("client_info").field("android_client_info").field("package_name") == packageName }
            ?: throw GradleException("$file has no Android app with package $packageName; download the one registered for it")
    val values =
        mapOf(
            "google_app_id" to client.field("client_info").field("mobilesdk_app_id"),
            "google_api_key" to (client.field("api_key") as? List<*>)?.firstOrNull().field("current_key"),
            "gcm_defaultSenderId" to project.field("project_number"),
            "project_id" to project.field("project_id"),
        )
    val missing = values.filterValues { it !is String || it.isEmpty() }.keys
    if (missing.isNotEmpty()) throw GradleException("$file lacks ${missing.joinToString()}; download it again from the Firebase console")
    return values.mapValues { it.value as String }
}
