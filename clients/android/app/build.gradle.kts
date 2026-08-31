plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Android 客户端统一放在 clients/android；版本仍只由工作区根 Cargo.toml 定义。
val workspaceVersion = file("../../../Cargo.toml").readText()
    .substringAfter("[workspace.package]")
    .substringBefore("\n[")
    .lineSequence()
    .first { it.trimStart().startsWith("version =") }
    .substringAfter('"')
    .substringBefore('"')

val semanticVersion = workspaceVersion.substringBefore('-').split('.').map(String::toInt)
require(semanticVersion.size == 3) { "Workspace version must use major.minor.patch" }
require(workspaceVersion == "0.2.0") {
    "The mobile v0.2 revision 1 contract requires workspace version 0.2.0; define a new epoch before bumping"
}

val releasePkcs12Path = providers
    .environmentVariable("MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_PATH")
    .orNull
    ?.takeIf { it.isNotBlank() }
val releasePkcs12Password = providers
    .environmentVariable("MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_PASSWORD")
    .orNull
    ?.takeIf { it.isNotEmpty() }

// `gradle build` also reaches Release tasks even though the requested task name
// does not contain "Release". Inspect the resolved graph so no indirect command
// can emit an unsigned formal APK.
gradle.taskGraph.whenReady {
    val buildsReleaseVariant = allTasks.any { task ->
        task.project == project && task.name.contains("Release", ignoreCase = true)
    }
    if (buildsReleaseVariant) {
        val signingPath = requireNotNull(releasePkcs12Path) {
            "Release tasks require MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_PATH"
        }
        requireNotNull(releasePkcs12Password) {
            "Release tasks require MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_PASSWORD"
        }
        require(file(signingPath).isFile) {
            "MEDIA_BACKUP_ANDROID_SIGNING_PKCS12_PATH must name a regular file"
        }
    }
}

android {
    namespace = "org.sarmg.mediabackup"
    compileSdk = 36

    defaultConfig {
        applicationId = "org.sarmg.mediabackup"
        minSdk = 26
        targetSdk = 36
        versionCode = semanticVersion[0] * 1_000_000 + semanticVersion[1] * 1_000 + semanticVersion[2]
        versionName = workspaceVersion

        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    signingConfigs {
        releasePkcs12Path?.let { signingPath ->
            releasePkcs12Password?.let { signingPassword ->
                create("release") {
                    storeFile = file(signingPath)
                    storePassword = signingPassword
                    keyAlias = "media-backup-android-release"
                    keyPassword = signingPassword
                    storeType = "PKCS12"
                }
            }
        }
    }

    buildTypes {
        getByName("release") {
            isDebuggable = false
            signingConfigs.findByName("release")?.let { signingConfig = it }
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
    testImplementation("junit:junit:4.13.2")
}
