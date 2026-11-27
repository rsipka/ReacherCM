
// Added to the app's build.gradle.kts after `tauri android init` (see the
// Android jobs in .github/workflows).
//
// The barcode scanner plugin uses ML Kit with only its "unbundled" barcode
// model, which Google Play Services downloads and runs. A phone without Play
// Services (GrapheneOS, LineageOS without GApps, /e/OS) never gets that model,
// so scanning a link QR code never worked there. Adding the bundled model puts
// the model and its native code inside the APK (about 2.5 MB per ABI); ML Kit
// then scans with it and needs no Play Services. The plugin's own ML Kit
// library stays: it holds the API (BarcodeScanning and friends) the plugin
// calls, and R8 fails without it.
dependencies {
    implementation("com.google.mlkit:barcode-scanning:17.3.0")
}
