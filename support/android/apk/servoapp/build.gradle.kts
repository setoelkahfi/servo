plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose)
    alias(libs.plugins.kotlin.serialization)
}

android {
    compileSdk { version = release(37) { minorApiLevel = 2 } }

    namespace = "org.servo.servoshell"

    defaultConfig {
        // The Play Store listing identity. This deliberately matches the Apple
        // bundle ID character for character, including the capital B: the two
        // storefronts are the same product, and an app ID can never be changed
        // once a build has been uploaded under it.
        applicationId = "xyz.smbcloud.Browser"
        minSdk = libs.versions.android.sdk.min.get().toInt()
        // Google Play refuses new uploads below API 36 from 31 August 2026.
        // Servo's own 34 was already a year past its deadline for the Play
        // Store; upstream ships APKs from CI and never sees that gate.
        targetSdk = 36

        // smbCloud Browser drives its own release numbering from
        // scripts/build-android.sh, which is where the storefront version
        // lives. Servo's date-derived versionCode stays as the fallback so a
        // plain `./mach package --android` still produces something installable.
        versionCode = System.getenv("SMB_VERSION_CODE")?.toInt() ?: generatedVersionCode
        versionName = System.getenv("SMB_VERSION_NAME") ?: "1.0.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }

    System.getenv("APK_SIGNING_KEY_STORE_PATH")?.let { storeFilePath ->
        signingConfigs {
            register("release") {
                storeFile = file(storeFilePath)
                storePassword = System.getenv("APK_SIGNING_KEY_STORE_PASS")!!
                keyAlias = System.getenv("APK_SIGNING_KEY_ALIAS")!!
                keyPassword = System.getenv("APK_SIGNING_KEY_PASS")!!
            }
        }
    }

    buildTypes {
        debug {}

        release {
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs["debug"]
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
        }

        // Custom build types

        val debug = getByName("debug")
        val release = getByName("release")

        register("armv7Debug") {
            initWith(debug)
            ndk { abiFilters.add(getNDKAbi("armv7")) }
        }
        register("armv7Release") {
            initWith(release)
            ndk { abiFilters.add(getNDKAbi("armv7")) }
        }
        register("arm64Debug") {
            initWith(debug)
            ndk { abiFilters.add(getNDKAbi("arm64")) }
        }
        register("arm64Release") {
            initWith(release)
            ndk { abiFilters.add(getNDKAbi("arm64")) }
        }
        register("x86Debug") {
            initWith(debug)
            ndk { abiFilters.add(getNDKAbi("x86")) }
        }
        register("x86Release") {
            initWith(release)
            ndk { abiFilters.add(getNDKAbi("x86")) }
        }
        register("x64Debug") {
            initWith(debug)
            ndk { abiFilters.add(getNDKAbi("x64")) }
        }
        register("x64Release") {
            initWith(release)
            ndk { abiFilters.add(getNDKAbi("x64")) }
        }
    }
}

// Ignore default "debug" and "release" build types
androidComponents {
    beforeVariants {
        if (it.buildType == "release" || it.buildType == "debug") {
            it.enable = false
        }
    }
}

dependencies {
    implementation(project(":servoview"))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.material3.compose)
    implementation(libs.androidx.material3.compose.adaptive)
    implementation(libs.androidx.preference)
    implementation(libs.kotlinx.serialization.json)
}
