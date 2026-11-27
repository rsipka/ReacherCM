// The DeX patch for the manifest `tauri android init` writes
// (src-tauri/android/patch-manifest.mjs).
import assert from 'node:assert/strict';
import { patchManifest } from '../src-tauri/android/patch-manifest.mjs';

// The shape Tauri 2's template generates.
const TAURI = `<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <uses-permission android:name="android.permission.INTERNET" />
    <application
        android:icon="@mipmap/ic_launcher"
        android:label="@string/app_name"
        android:theme="@style/Theme.reacher_cm"
        android:usesCleartextTraffic="\${usesCleartextTraffic}">
        <activity
            android:configChanges="orientation|keyboardHidden|keyboard|screenSize|locale|smallestScreenSize|screenLayout|uiMode"
            android:launchMode="singleTask"
            android:label="@string/main_activity_title"
            android:name=".MainActivity"
            android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
`;

const out = patchManifest(TAURI);
const changes = /android:configChanges="([^"]*)"/.exec(out)[1].split('|');
for (const c of ['density', 'navigation', 'uiMode', 'screenSize', 'smallestScreenSize', 'screenLayout', 'keyboard', 'keyboardHidden']) {
	assert.ok(changes.includes(c), `configChanges has ${c}`);
}
assert.equal(new Set(changes).size, changes.length, 'no value twice');
assert.match(out, /android:resizeableActivity="true"/);
assert.match(out, /<activity[^>]*>\s*<layout android:defaultWidth="1200dp"[^>]*\/>/);
assert.match(out, /<application[^>]*>\s*<meta-data android:name="com.samsung.android.keepalive.density" android:value="true" \/>/);
assert.match(out, /<uses-permission android:name="android.permission.MANAGE_EXTERNAL_STORAGE" \/>/);
assert.match(out, /READ_EXTERNAL_STORAGE" android:maxSdkVersion="29"/);
assert.match(out, /WRITE_EXTERNAL_STORAGE" android:maxSdkVersion="29"/);
assert.match(out, /<application android:requestLegacyExternalStorage="true"/);
assert.ok(out.indexOf('MANAGE_EXTERNAL_STORAGE') < out.indexOf('<application'), 'permissions before <application>');
assert.equal(patchManifest(out), out, 'running it twice changes nothing');
assert.throws(() => patchManifest('<manifest><application></application></manifest>'), /MainActivity/);
console.log('android manifest: ok');
