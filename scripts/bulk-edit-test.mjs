/**
 * Tests the rules of the bulk session editor (#lib/sessions/bulk-edit): which
 * sessions take which field, and how values that differ are summed up.
 *
 * Run: node --import ./scripts/resolve-lib.mjs scripts/bulk-edit-test.mjs
 */

import {
	appliesTo,
	changedFields,
	countApplying,
	describeCounts,
	hasOwnPassword,
	loginFromCredential,
	parseTags,
	summarize,
	vaultsOf
} from '../src/lib/sessions/bulk-edit.ts';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name} ${extra}`);
	}
};

const base = { id: 'x', name: 'x', host: 'h', port: 22, username: 'root', folder_id: null, tags: [], auth_method: { type: 'Password', password: 'pw' } };
const ssh = { ...base, id: 'ssh' };
const rdp = { ...base, id: 'rdp', kind: 'rdp', port: 3389, username: 'admin', vault_id: 'team' };
const wsl = { ...base, id: 'wsl', kind: 'wsl', auth_method: { type: 'Agent' } };
const vnc = { ...base, id: 'vnc', kind: 'vnc' };
const keyed = { ...base, id: 'key', auth_method: { type: 'Key', key_id: 'k1' } };
const linked = { ...base, id: 'linked', credential_id: 'c1', credential: { id: 'c1', name: 'ops', folder_id: null, missing: false } };
const viaFolder = { ...base, id: 'folder', credential: { id: 'c2', name: 'f', folder_id: 'f1', missing: false } };
const all = [ssh, rdp, wsl, vnc, keyed, linked, viaFolder];

console.log('Fields by kind');
check('domain is RDP only', countApplying('domain', all) === 1 && appliesTo('domain', rdp));
check('terminal colours skip RDP and VNC', !appliesTo('terminal_theme', rdp) && !appliesTo('terminal_theme', vnc) && appliesTo('terminal_theme', wsl));
check('the cursor skips RDP and VNC', !appliesTo('cursor_style', rdp) && !appliesTo('cursor_blink', vnc) && appliesTo('cursor_blink', wsl));
check('WSL has no login or port', !appliesTo('username', wsl) && !appliesTo('port', wsl) && !appliesTo('password', wsl));
check('a credential is offered for SSH, RDP and Telnet, not VNC', appliesTo('credential', ssh) && appliesTo('credential', rdp) && !appliesTo('credential', vnc));
check('tags and notes apply to every session', countApplying('tags', all) === all.length && countApplying('notes', all) === all.length);

console.log('Logins from a credential');
check('a password goes only to password sessions', !appliesTo('password', keyed) && appliesTo('password', vnc));
check('a linked credential keeps its username', !appliesTo('username', linked) && !appliesTo('password', linked));
check('a folder credential counts too', loginFromCredential(viaFolder) && !appliesTo('username', viaFolder));
check("choosing the session's own login frees the username", appliesTo('username', linked, { set: 'own' }));
check('linking a credential takes the username', !appliesTo('username', ssh, { set: 'c9' }) && appliesTo('username', vnc, { set: 'c9' }));
check('a missing credential leaves the own login', appliesTo('username', { ...linked, credential: { ...linked.credential, missing: true } }));

console.log('Phase 2 fields');
check('login method, jump hosts, proxy and shell are SSH only', ['auth', 'jump_chain', 'proxy', 'shell'].every((f) => appliesTo(f, ssh) && !appliesTo(f, rdp) && !appliesTo(f, wsl)));
check('a credential keeps its login method', !appliesTo('auth', linked));
check('agent keys only for key sessions', appliesTo('try_agent_keys', keyed) && !appliesTo('try_agent_keys', ssh));
check('switching to a key makes agent keys apply', appliesTo('try_agent_keys', ssh, undefined, 'Key'));
check('switching to a password makes the password apply', appliesTo('password', keyed, undefined, 'Password') && !appliesTo('password', keyed));
check('a switch does not reach a credential login', !appliesTo('password', linked, undefined, 'Password'));
check('monitoring and tmux on SSH and WSL', appliesTo('tmux', wsl) && appliesTo('monitoring', ssh) && !appliesTo('tmux', rdp));
check('VNC tunnel on VNC only', appliesTo('via_session', vnc) && !appliesTo('via_session', ssh));
check('Telnet options on Telnet only', appliesTo('telnet', { ...base, kind: 'telnet' }) && !appliesTo('telnet', ssh));
check('custom fields everywhere', countApplying('custom_fields', all) === all.length);
check(
	'phase 2 changes are listed',
	changedFields({ auth: { type: 'Agent' }, fields_remove: ['x'], telnet: {}, tmux: { mouse: true }, via_session: 'clear' }).join() === 'auth,custom_fields,tmux,via_session'
);

console.log('Newer fields');
const multi = { ...base, id: 'multi', alt_addresses: [{ host: '10.0.0.2' }] };
const woken = { ...base, id: 'wol', wol: { mac: '00:11:22:33:44:55' } };
check('WireGuard on SSH, RDP and VNC, not WSL', appliesTo('wireguard', ssh) && appliesTo('wireguard', rdp) && appliesTo('wireguard', vnc) && !appliesTo('wireguard', wsl));
check('connect mode only with other addresses', appliesTo('connect_mode', multi) && !appliesTo('connect_mode', ssh) && appliesTo('address_check', multi));
check('terminal settings and files on SSH only', appliesTo('terminal', ssh) && !appliesTo('terminal', wsl) && appliesTo('files', ssh) && !appliesTo('files', rdp));
check('RDP settings on RDP only', appliesTo('rdp', rdp) && !appliesTo('rdp', ssh));
check('Wake-on-LAN only where a MAC is set', appliesTo('wol', woken) && !appliesTo('wol', ssh));
check('favorite everywhere, OS icon not on RDP or WSL', countApplying('favorite', all) === all.length && appliesTo('os_icon', vnc) && !appliesTo('os_icon', rdp) && !appliesTo('os_icon', wsl));
check(
	'new changes are listed',
	changedFields({ os_icon: 'clear', rdp: { admin: true }, terminal: {}, wireguard: { set: 'wg' }, favorite: false, wol: { port: 'clear' } }).join() ===
		'wireguard,rdp,wol,favorite,os_icon'
);

console.log('Summaries');
const users = summarize([ssh, rdp, linked, { ...base, username: '' }], (s) => s.username);
check('mixed values have no common value', users.same === null);
check('most common first', users.counts[0][0] === 'root' && users.counts[0][1] === 2, JSON.stringify(users.counts));
check('described with counts', describeCounts(users, '(none)') === 'root ×2, (none) ×1, admin ×1', describeCounts(users, '(none)'));
check('one value is the common one', summarize([ssh, keyed], (s) => s.port).same === '22');
check('long lists are cut', describeCounts(summarize([1, 2, 3, 4, 5], (n) => n), '', 3).endsWith('…'));
check('own password detected', hasOwnPassword(ssh) && !hasOwnPassword(keyed) && hasOwnPassword({ ...base, auth_method: { type: 'Password', password_ref: { provider: 'bitwarden', entry_id: 'e' } } }));

console.log('Patches');
check('tags split on commas and spaces', JSON.stringify(parseTags(' prod, web  eu,prod ')) === '["prod","web","eu"]');
check('an empty patch changes nothing', changedFields({}).length === 0);
check('changed fields in order', changedFields({ history: 'clear', username: 'u', tags_remove: ['a'] }).join() === 'username,tags,history');
check('cursor fields come last', changedFields({ cursor_blink: 'clear', cursor_style: { set: 'bar' }, history: 'clear' }).join() === 'history,cursor_style,cursor_blink');
check('empty tag lists are not a change', !changedFields({ tags_add: [], tags_remove: [] }).includes('tags'));
check('vaults are told apart', vaultsOf(all).length === 2 && vaultsOf([ssh, keyed]).length === 1);

if (failures > 0) {
	console.log(`\n${failures} failed`);
	process.exit(1);
}
console.log('\nAll passed');
