import java.util.Properties
import java.io.FileInputStream

// ── 签名与版本矩阵 ──────────────────────────────────────────────
// 读 tauri.properties 与 keystore.properties，避免版本号与密钥在多处漂移。
val tauriProperties = Properties().apply {
    val propFile = rootProject.file("tauri.properties")
    if (propFile.exists()) {
        load(FileInputStream(propFile))
    }
}

val keystorePropertiesFile = rootProject.file("keystore.properties")
val keystoreProperties = Properties().apply {
    if (keystorePropertiesFile.exists()) {
        load(FileInputStream(keystorePropertiesFile))
    }
}

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    // 序列化插件：MediaSession 的命令参数走 Kotlinx serialization，
    // 手写 bundle 解析是 bug 温床。
    id("org.jetbrains.kotlin.plugin.serialization") version "1.9.24"
}

android {
    // Media3 1.4+ 要求 compileSdk 34。低于它会拿到一堆莫名其妙的编译错误。
    compileSdk = 34
    ndkVersion = "28.2.13676358"
    namespace = "media.whatmedia.client"

    defaultConfig {
        applicationId = "media.whatmedia.client"
        // 24 = Android 7.0。再往下 Media3 跑不动（缺 MediaSession 的关键 API）。
        minSdk = 24
        // 34：targetSdk 低于它会触发系统的兼容模式告警，
        // 且在 Android 14+ 上后台启动限制会更严。
        targetSdk = 34
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "0.1.0")

        // 只保留中英文。首发 APK 里带一堆没用语言包是纯浪费。
        resourceConfigurations += listOf("zh", "en")

        // 通知栏 / MediaSession 的媒体样式需要
        // `androidx.media3.session` 的 MediaStyle，无需额外配置。
    }

    signingConfigs {
        create("release") {
            val storeFileName = keystoreProperties.getProperty("storeFile")
            val keystoreFile = if (storeFileName != null) rootProject.file(storeFileName) else null
            if (keystoreFile != null && keystoreFile.exists()) {
                storeFile = keystoreFile
                storePassword = keystoreProperties.getProperty("storePassword")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
            } else {
                initWith(getByName("debug"))
            }
        }
    }

    splits {
        abi {
            isEnable = true
            reset()
            include("armeabi-v7a", "arm64-v8a", "x86", "x86_64")
            isUniversalApk = true
        }
    }

    buildTypes {
        getByName("debug") {
            // 调试包不打 minify，否则 Rust panic 栈全被混淆掉。
            isMinifyEnabled = false
            // 让 `run-as` 能取到 APK 里的 so，便于现场抓 native crash。
            isJniDebuggable = true
        }
        getByName("release") {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            signingConfig = signingConfigs.getByName("release")
        }
    }



    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
        // ExoPlayer 的 -Werror 会把"未检查的 cast"也当错误，徒增噪音。
        freeCompilerArgs += listOf("-opt-in=androidx.media3.common.util.UnstableApi")
    }

    // 把仓库根 plugins/ 下的 Kotlin 源码吃进来。
    // 为什么用 srcDirs 而不是建 Gradle 模块：见 settings.gradle 的注释。
    sourceSets {
        getByName("main").java.srcDirs(
            "src/main/java",
            rootProject.file("../../../../plugins/tauri-plugin-media3/android/src/main/java"),
        )
    }

    // Media3 大量使用 @UnstableApi。逐个 suppress 会让代码糊掉，
    // 这里统一在 lint 层关掉。
    lint {
        disable += "UnstableApiUsage"
        // Tauri 自带的 WebView 资源会触发一堆 lint 告警，与我们的代码无关。
        abortOnError = false
        checkReleaseBuilds = false
    }

    packaging {
        resources {
            // Rust 动态库：避免重复条目导致打包失败
            excludes += "META-INF/**"
        }
        jniLibs {
            // 保留调试符号到 debug 包，便于 ndk-stack
            keepDebugSymbols += "**/*.so"
        }
    }
}

repositories {
    google()
    mavenCentral()
}

dependencies {
    // Tauri Android 运行时
    implementation(project(":tauri-android"))
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.activity:activity-ktx:1.9.2")
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.webkit:webkit:1.12.0")
    implementation("androidx.constraintlayout:constraintlayout:2.2.0")
    implementation("com.google.android.material:material:1.12.0")

    // ── Media3 / ExoPlayer ────────────────────────────────
    // 三个模块必须同版本，否则会在运行时报 NoSuchMethodError
    // （media3 的 ABI 在 1.x 之间并不稳定）。
    val media3Version = "1.4.1"
    implementation("androidx.media3:media3-exoplayer:$media3Version")
    implementation("androidx.media3:media3-ui:$media3Version")
    implementation("androidx.media3:media3-session:$media3Version")
    // HLS / DASH：我们服务端只出 HLS，DASH 暂时不引（体积）。
    implementation("androidx.media3:media3-exoplayer-hls:$media3Version")
    // 外挂字幕解码（SRT / ASS / VTT）
    implementation("androidx.media3:media3-extractor:$media3Version")
    implementation("androidx.media3:media3-common:$media3Version")
    // 解码器选择：让 ExoPlayer 优先选硬件解码器
    implementation("androidx.media3:media3-datasource-okhttp:$media3Version")

    implementation("org.jetbrains.kotlinx:kotlinx-serialization-core:1.6.3")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.6.3")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")

    // MediaSession 的后台播放服务需要 lifecycle / media 兼容库。
    implementation("androidx.lifecycle:lifecycle-process:2.8.6")
    implementation("androidx.mediarouter:mediarouter:1.7.0")
}

configurations.all {
    resolutionStrategy.eachDependency {
        if (requested.group == "org.jetbrains.kotlinx" && requested.name.startsWith("kotlinx-serialization")) {
            useVersion("1.6.3")
            because("Kotlin 1.9.24 compatibility")
        }
    }
}


// ── Tauri 的 Rust 构建钩子 ────────────────────────────────
//
// Tauri 在 Gradle 里注册了 `rustXxx` 任务来交叉编译 Rust。
// 手写工程时这些任务由 `tauri_build` 注入，这里只负责声明依赖顺序：
// 必须先有 so，才能打包 APK。
//
// 注意：如果 `tauri android init` 补生成了这些任务的声明，
// 本段应当删掉，以生成物为准 —— 重复注册同名任务是硬错误。
afterEvaluate {
    tasks.findByName("tauriBuildRust")?.let { rustTask ->
        tasks.matching { it.name.startsWith("merge") && it.name.contains("JniLib") }
            .configureEach {
                dependsOn(rustTask)
            }
    }
}

apply(from = "tauri.build.gradle.kts")

tasks.register("assembleUniversalRelease") {
    dependsOn("assembleRelease")
    doLast {
        val srcDir = layout.buildDirectory.dir("outputs/apk/release").get().asFile
        val dstDir = layout.buildDirectory.dir("outputs/apk/universal/release").get().asFile
        if (srcDir.exists()) {
            dstDir.mkdirs()
            srcDir.listFiles { file -> file.extension == "apk" }?.forEach { apk ->
                val targetFile = File(dstDir, apk.name.replace("-release", "-universal-release"))
                apk.copyTo(targetFile, overwrite = true)
                apk.copyTo(File(dstDir, apk.name), overwrite = true)
                println("Universal APK mirrored to: ${targetFile.absolutePath}")
            }
            val universalApk = File(dstDir, "app-universal-release.apk")
            if (universalApk.exists()) {
                val brandedApk = File(dstDir, "WhatMedia-v${android.defaultConfig.versionName}-release.apk")
                universalApk.copyTo(brandedApk, overwrite = true)
                println("Branded Release APK available at: ${brandedApk.absolutePath}")
            }
        }
    }
}

tasks.register("assembleUniversalDebug") {
    dependsOn("assembleDebug")
}

tasks.register("bundleUniversalRelease") {
    dependsOn("bundleRelease")
    doLast {
        val srcDir = layout.buildDirectory.dir("outputs/bundle/release").get().asFile
        val dstDir = layout.buildDirectory.dir("outputs/bundle/universal/release").get().asFile
        if (srcDir.exists()) {
            dstDir.mkdirs()
            srcDir.listFiles { file -> file.extension == "aab" }?.forEach { aab ->
                val targetFile = File(dstDir, aab.name.replace("-release", "-universal-release"))
                aab.copyTo(targetFile, overwrite = true)
                aab.copyTo(File(dstDir, aab.name), overwrite = true)
                println("Universal AAB mirrored to: ${targetFile.absolutePath}")
            }
            val universalAab = File(dstDir, "app-universal-release.aab")
            if (universalAab.exists()) {
                val brandedAab = File(dstDir, "WhatMedia-v${android.defaultConfig.versionName}-release.aab")
                universalAab.copyTo(brandedAab, overwrite = true)
                println("Branded Release AAB available at: ${brandedAab.absolutePath}")
            }
        }
    }
}

tasks.register("bundleUniversalDebug") {
    dependsOn("bundleDebug")
}








