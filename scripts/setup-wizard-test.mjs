/**
 * The setup wizard: which screens it shows on a first run and when run again
 * from Settings, where Skip goes, and that every text its screens ask for
 * exists in English (a missing key shows as the key itself).
 *
 * Run: node --import ./scripts/resolve-lib.mjs scripts/setup-wizard-test.mjs
 */

import { readFileSync, readdirSync } from 'node:fs';
import { wizardSteps, skipTarget } from '#lib/setup/wizard-steps.js';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name}${extra ? '\n        ' + extra : ''}`);
	}
};
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

console.log('\nSteps');
const first = wizardSteps({ rerun: false });
const again = wizardSteps({ rerun: true });
check(
	'first run asks everything, in order',
	same(first, ['personalize', 'sessions', 'logging', 'security', 'plugins', 'account', 'complete']),
	JSON.stringify(first)
);
check('run again offers account sync and leaves out plugins', same(again, ['personalize', 'sessions', 'logging', 'security', 'account', 'complete']), JSON.stringify(again));
check('both end on the summary', first.at(-1) === 'complete' && again.at(-1) === 'complete');
check('no Turso storage step', !first.includes('storage') && !again.includes('storage'));
check('no step twice', new Set(first).size === first.length);
check('Skip goes to the summary', first[skipTarget(first)] === 'complete' && again[skipTarget(again)] === 'complete');

console.log('\nTexts');
const en = JSON.parse(readFileSync(new URL('../src/lib/i18n/locales/en.json', import.meta.url), 'utf8'));
const dir = new URL('../src/lib/components/setup/', import.meta.url);
const missing = [];
for (const file of readdirSync(dir).filter((f) => f.endsWith('.svelte'))) {
	const src = readFileSync(new URL(file, dir), 'utf8');
	for (const m of src.matchAll(/['"]((?:setup|settings|history|themes|security|sync_account)\.[a-z0-9_]+)['"]/g)) {
		if (!(m[1] in en)) missing.push(`${file}: ${m[1]}`);
	}
}
check('every key the setup screens use is in en.json', missing.length === 0, missing.join(', '));

console.log(failures ? `\n${failures} failed` : '\nAll passed');
process.exit(failures ? 1 : 0);
