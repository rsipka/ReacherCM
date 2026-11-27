/**
 * Tests which sessions start in tmux, their names and the reconnect
 * schedule, in Node.
 *
 * Run: node scripts/tmux-test.mjs
 */

import {
	validTmuxName,
	DEFAULT_TMUX_NAME, legacyTmuxName,
	folderTmux,
	tmuxFor,
	autoReconnectDelay,
	needsUser,
	AUTO_RECONNECT_FOR_MS,
	newViewTag,
	tmuxWindowFromTitle
} from '../src/lib/terminal/tmux.ts';
import { osc52Text, MAX_OSC52_BASE64 } from '../src/lib/terminal/osc52.ts';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name}${extra ? '\n        ' + extra : ''}`);
	}
};
const section = (s) => console.log(`\n${s}`);

section('names');
check('plain names pass', validTmuxName('reacher-web_01'));
check('spaces, quotes, colons and dots fail', ['a b', "a'b", 'a:b', 'a.b', '', '-x', 'x'.repeat(65)].every((n) => !validTmuxName(n)));
check('the default name is reachercm-tmux', DEFAULT_TMUX_NAME === 'reachercm-tmux' && validTmuxName(DEFAULT_TMUX_NAME));
check('the old name came from the session name', legacyTmuxName('Web 01 (prod)') === 'reachercm-web-01-prod', legacyTmuxName('Web 01 (prod)'));
check('accents are dropped', legacyTmuxName('Účetní server') === 'reachercm-ucetni-server', legacyTmuxName('Účetní server'));
check('nothing usable left: plain reachercm', legacyTmuxName('—') === 'reachercm');
check('long names stay within 64', legacyTmuxName('x'.repeat(100)).length === 64);
check('a long name stays valid', validTmuxName(legacyTmuxName('x '.repeat(80))));

section('folders');
const folders = [
	{ id: 'top', name: 'Top', parent_id: null, vault_id: null, tmux: true },
	{ id: 'mid', name: 'Mid', parent_id: 'top', vault_id: null },
	{ id: 'off', name: 'Off', parent_id: 'top', vault_id: null, tmux: false },
	{ id: 'loop', name: 'Loop', parent_id: 'loop', vault_id: null }
];
check('the nearest folder that says decides', folderTmux('mid', folders) === true && folderTmux('off', folders) === false);
check('no folder: unset', folderTmux(null, folders) === null);
check('a folder loop ends', folderTmux('loop', folders) === null);

section('sessions');
const s = (over) => ({ name: 'Web', kind: 'ssh', folder_id: null, tmux: null, ...over });
check('off by default', tmuxFor(s({}), folders) === null);
check('attach on the session', tmuxFor(s({ tmux: { mode: 'attach' } }), folders)?.name === 'reachercm-tmux');
check('with the old default name as legacy', tmuxFor(s({ tmux: { mode: 'attach' } }), folders)?.legacy === 'reachercm-web');
check('an own name has no legacy', tmuxFor(s({ tmux: { mode: 'attach', name: 'main' } }), folders)?.legacy === undefined);
check('its own name', tmuxFor(s({ tmux: { mode: 'attach', name: 'main' } }), folders)?.name === 'main');
check('a bad own name falls back to the default', tmuxFor(s({ tmux: { mode: 'attach', name: 'a b' } }), folders)?.name === 'reachercm-tmux');
check('the folder turns it on', tmuxFor(s({ folder_id: 'mid' }), folders)?.name === 'reachercm-tmux');
check('inherit keeps a name of its own', tmuxFor(s({ folder_id: 'mid', tmux: { mode: 'inherit', name: 'x' } }), folders)?.name === 'x');
check('off on the session beats the folder', tmuxFor(s({ folder_id: 'mid', tmux: { mode: 'off' } }), folders) === null);
check('a mode from a newer version follows the folder', tmuxFor(s({ folder_id: 'off', tmux: { mode: 'pick-later' } }), folders) === null);
check('never for Telnet', tmuxFor(s({ kind: 'telnet', tmux: { mode: 'attach' } }), folders) === null);
check('ask on SSH asks', tmuxFor(s({ tmux: { mode: 'ask' } }), folders)?.ask === true);
check('WSL runs tmux, never asks', (() => { const w = tmuxFor(s({ kind: 'wsl', tmux: { mode: 'ask' } }), folders); return w !== null && !w.ask; })());
check('mouse is passed on', tmuxFor(s({ tmux: { mode: 'attach', mouse: true } }), folders)?.mouse === true);
check('no mouse unless asked', tmuxFor(s({ tmux: { mode: 'attach' } }), folders)?.mouse === undefined);
check('an old save without a kind is SSH', tmuxFor({ name: 'Web', folder_id: null, tmux: { mode: 'attach' } }, folders) !== null);

section('reconnect');
check('2, 5, 10, 30 seconds, then every minute',
	[0, 1, 2, 3, 4, 9].map((a) => autoReconnectDelay(a, 0)).join() === '2000,5000,10000,30000,60000,60000');
check('it stops after ten minutes', autoReconnectDelay(5, AUTO_RECONNECT_FOR_MS) === null);
check('a refused login needs the user', needsUser('Authentication failed') && needsUser('Password required for password auth'));
check('a network error does not', !needsUser('Connection timed out') && !needsUser('No route to host'));

section('OSC 52');
const b64 = (txt) => Buffer.from(txt, 'utf8').toString('base64');
check('copies text', osc52Text(`c;${b64('hello')}`) === 'hello');
check('any selection letter, or none', osc52Text(`p;${b64('x')}`) === 'x' && osc52Text(`;${b64('x')}`) === 'x');
check('UTF-8 survives', osc52Text(`c;${b64('příliš žluťoučký 🙂')}`) === 'příliš žluťoučký 🙂');
check('a query is never answered', osc52Text('c;?') === null);
check('a clear does nothing', osc52Text('c;') === null);
check('not base64: nothing', osc52Text('c;%%%') === null && osc52Text('no-semicolon') === null);
check('too long: nothing', osc52Text('c;' + 'A'.repeat(MAX_OSC52_BASE64 + 4)) === null);
check('broken UTF-8: nothing', osc52Text('c;' + Buffer.from([0xff, 0xfe]).toString('base64')) === null);

// Window tabs: a view's tag and the title it sets.
{
	const tags = new Set(Array.from({ length: 50 }, () => newViewTag()));
	check('view tags are lowercase letters and digits', [...tags].every((x) => /^[a-z0-9]{1,16}$/.test(x)), [...tags].join(' '));
	check('view tags differ', tags.size > 45, String(tags.size));
	const w = tmuxWindowFromTitle('@12 logs: app');
	check('a view title names its window', w?.id === '@12' && w?.name === 'logs: app', JSON.stringify(w));
	check('a shell title is not a window', tmuxWindowFromTitle('radek@web: ~') === null);
	check('an empty window name is kept', tmuxWindowFromTitle('@3 ')?.name === '');
}

console.log(failures ? `\n${failures} failed` : '\nAll passed');
process.exit(failures ? 1 : 0);
