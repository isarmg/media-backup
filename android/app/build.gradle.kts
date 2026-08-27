plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

val workspaceVersion = file("../../Cargo.toml").readText()
    .substringAfter("[workspace.package]")
    .substringBefore("\n[")
    .lineSequence()
    .first { it.trimStart().startsWith("version =") }
    .substringAfter('"')
    .substringBefore('"')

val semanticVersion = workspaceVersion.substringBefore('-').split('.').map(String::toInt)
require(semanticVersion.size == 3) { "Workspace version must use major.minor.patch" }

android {
    namespace = "com.example.photobackup"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.example.photobackup"
        minSdk = 26
        targetSdk = 36
        versionCode = semanticVersion[0] * 1_000_000 + semanticVersion[1] * 1_000 + semanticVersion[2]
        versionName = workspaceVersion

        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        jniLibs.useLegacyPackaging = false
        resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.06.01")
    implementation(composeBom)
    androidTestImplementation(composeBom)
    implementation("androidx.activity:activity-compose:1.12.3")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("androidx.work:work-runtime-ktx:2.11.2")
    implementation("androidx.security:security-crypto:1.1.0")
    implementation("com.squareup.okhttp3:okhttp:5.1.0")
}
