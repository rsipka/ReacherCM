/**
 * Tests the pure parts of session screenshots (#lib/terminal/snapshot):
 * the 256-colour palette and the file name.
 *
 * Run: node --import ./scripts/resolve-lib.mjs scripts/snapshot-test.mjs
 */

import { palette, screenshotName } from '#lib/terminal/snapshot.js';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name}${extra ? '\n        ' + extra : ''}`);
	}
};

const p = palette({ red: '#ff0000' });
check('256 colours', p.length === 256);
check('theme colour used', p[1] === '#ff0000');
check('missing theme colour has a default', p[0] === '#2e3436');
check('cube start', p[16] === '#000000');
check('cube end', p[231] === '#ffffff');
check('cube middle', p[16 + 36 * 1 + 6 * 2 + 3] === '#5f87af');
check('grey ramp', p[232] === '#080808' && p[255] === '#eeeeee');
check('extended colours override', palette({ extendedAnsi: ['#123456'] })[16] === '#123456');

const at = new Date(2026, 10, 23, 21, 5, 9);
check('name and time', screenshotName('web-01', at) === 'web-01 2026-11-23 21-05-09.png', screenshotName('web-01', at));
check('unsafe characters replaced', screenshotName('a/b:c*d', at) === 'a_b_c_d 2026-11-23 21-05-09.png', screenshotName('a/b:c*d', at));
check('empty name', screenshotName('  ', at) === 'screenshot 2026-11-23 21-05-09.png');
check('long name cut', screenshotName('x'.repeat(200), at).length === 80 + ' 2026-11-23 21-05-09.png'.length);
check('accents kept', screenshotName('Účetní server', at).startsWith('Účetní server '));

if (failures) {
	console.log(`\n${failures} failed`);
	process.exit(1);
}
console.log('\nall passed');
