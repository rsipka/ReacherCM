# Added to the app's proguard-rules.pro after `tauri android init` (see the
# Android jobs in .github/workflows).
#
# ML Kit (the QR code scanner) finds its parts by creating its component
# registrars through reflection. R8 removed their empty constructors in the
# release build, so ML Kit started without its barcode part and the app
# crashed as soon as the camera opened (NullPointerException in
# BarcodeScannerPlugin).
-keep class * implements com.google.firebase.components.ComponentRegistrar {
    public <init>();
}
-keep class com.google.mlkit.** { *; }
-keep class com.google.android.gms.internal.mlkit_vision_barcode.** { *; }
-keep class com.google.android.gms.internal.mlkit_vision_common.** { *; }
-keep class com.google.android.gms.internal.mlkit_common.** { *; }
# The bundled barcode model (see no-gms.gradle.kts) reaches its native code
# through these classes.
-keep class com.google.android.gms.internal.mlkit_vision_barcode_bundled.** { *; }
