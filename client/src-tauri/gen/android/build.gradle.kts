// 根构建脚本：只声明插件版本，不应用。
//
// 版本集中在这里，app/ 与 :tauri-plugin-media3 里写 `id(...)` 不带版本。
// 分散写版本的后果是两个模块拿到不同的 AGP，报出来的错
// 是 "Unresolved reference: android" 之类跟版本毫无关系的错。
plugins {
    id("com.android.application") apply false
    id("com.android.library") apply false
    id("org.jetbrains.kotlin.android") version "1.9.24" apply false
    id("org.jetbrains.kotlin.plugin.serialization") version "1.9.24" apply false
}


tasks.register<Delete>("clean") {
    delete(rootProject.layout.buildDirectory)
}
