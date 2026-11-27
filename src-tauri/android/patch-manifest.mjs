// Run after `tauri android init` (see the Android jobs in .github/workflows):
//   node src-tauri/android/patch-manifest.mjs [path/to/AndroidManifest.xml]
//
// Samsung DeX and other desktop modes:
// - Starting or stopping DeX, or moving the window between the phone and a
//   monitor, changes the screen density (and the navigation). The activity
//   Tauri generates does not list those, so Android started it again and
//   every open session dropped. They are added to configChanges, and
//   MainActivity hears about them in onConfigurationChanged instead.
// - Samsung's keepalive flag keeps the process when the density changes.
// - A free-form window (DeX, Android's desktop windowing) opens at a desktop
//   size instead of a phone's, and cannot be made too small to use.
import { readFileSync, writeFileSync } from 'node:fs';

const path = process.argv[2] ?? 'src-tauri/gen/android/app/src/main/AndroidManifest.xml';

/** Everything a desktop mode can change under the app. */
const CONFIG_CHANGES = [
	'orientation',
	'keyboardHidden',
	'keyboard',
	'screenSize',
	'locale',
	'smallestScreenSize',
	'screenLayout',
	'uiMode',
	'density',
	'navigation'
];

const LAYOUT =
	'<layout android:defaultWidth="1200dp" android:defaultHeight="760dp" android:gravity="center" android:minWidth="360dp" android:minHeight="400dp" />';

const DEX_META = [
	'<meta-data android:name="com.samsung.android.keepalive.density" android:value="true" />',
	'<meta-data android:name="com.samsung.android.multidisplay.keep_process_alive" android:value="true" />'
];

export function patchManifest(xml) {
	const activity = /<activity\b[^>]*android:name="\.MainActivity"[^>]*>/.exec(xml);
	if (!activity) throw new Error('MainActivity not found in the manifest');
	let tag = activity[0];

	const current = /android:configChanges="([^"]*)"/.exec(tag);
	const have = current ? current[1].split('|').filter(Boolean) : [];
	const all = [...have, ...CONFIG_CHANGES.filter((c) => !have.includes(c))].join('|');
	tag = current ? tag.replace(current[0], `android:configChanges="${all}"`) : tag.replace(/<activity\b/, `<activity android:configChanges="${all}"`);
	if (!/android:resizeableActivity=/.test(tag)) tag = tag.replace(/<activity\b/, '<activity android:resizeableActivity="true"');

	let out = xml.replace(activity[0], tag);
	if (!/<layout\b/.test(out)) out = out.replace(tag, `${tag}\n            ${LAYOUT}`);

	const app = /<application\b[^>]*>/.exec(out);
	if (!app) throw new Error('<application> not found in the manifest');
	const missing = DEX_META.filter((m) => !out.includes(/android:name="([^"]+)"/.exec(m)[0]));
	if (missing.length) out = out.replace(app[0], `${app[0]}\n        ${missing.join('\n        ')}`);
	return out;
}

if (import.meta.url === `file://${process.argv[1]}`) {
	writeFileSync(path, patchManifest(readFileSync(path, 'utf8')));
	console.log(`Patched ${path} for desktop modes (DeX).`);
}
