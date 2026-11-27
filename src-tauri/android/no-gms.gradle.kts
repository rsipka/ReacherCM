
// Added to the app's build.gradle.kts after `tauri android init` (see the
// Android jobs in .github/workflows).
//
// The barcode scanner plugin asks for ML Kit's "unbundled" barcode model
// (play-services-mlkit-barcode-scanning), which Google Play Services downloads
// and runs. A phone without Play Services (GrapheneOS, LineageOS without
// GApps, /e/OS) never gets that model, so scanning a link QR code never
// worked there. The bundled model has the same API, ships inside the APK
// (about 2.5 MB per ABI) and needs no Play Services, so it is swapped in here.
configurations.all {
    exclude(group = "com.google.android.gms", module = "play-services-mlkit-barcode-scanning")
}
dependencies {
    implementation("com.google.mlkit:barcode-scanning:17.3.0")
}
