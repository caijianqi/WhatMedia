# R8 规则。
#
# 三条都必须留着，各对应一个真实事故：

# 1. Tauri 的 Rust 插件靠 JNI 从 Kotlin 侧反射调进来。
#    混淆后方法名变了 → 运行时 UnsatisfiedLinkError，且只在 release 包出现。
-keep class media.whatmedia.client.** { *; }
-keep class app.tauri.** { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}

# 2. Media3 的 MediaSession 用反射查命令常量。
-keep class androidx.media3.** { *; }
-keep class androidx.media3.session.** { *; }
-dontwarn androidx.media3.**

# 3. Kotlinx serialization 生成的序列化器在 release 下会被剥掉。
-keepclassmembers class **$Serializer { *; }
-keepclassmembers class * implements kotlinx.serialization.KSerializer { *; }
-keep,includedescriptorclasses class media.whatmedia.**$$serializer { *; }

# 日志在 release 里去掉，但别去掉异常栈。
-assumenosideeffects class android.util.Log {
    public static int d(...);
    public static int v(...);
    public static int i(...);
}
