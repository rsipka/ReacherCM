/**
 * Tests which of a session's addresses a connect uses (#lib/sessions/addresses):
 * their order, which go through the session's route, and when a failure moves
 * on to the next one.
 *
 * Run: node --import ./scripts/resolve-lib.mjs scripts/addresses-test.mjs
 */

import { addressKey, addressLabelled, autoOrder, checksAddresses, plainOrder, connectMode, hasAltAddresses, isUnreachable, sessionAddresses } from '../src/lib/sessions/addresses.ts';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name} ${extra}`);
	}
};

const plain = { host: 'server.example.com', port: 22 };
check('no other addresses: main only', !hasAltAddresses(plain) && connectMode(plain) === 'main' && sessionAddresses(plain).length === 1);

const jump = [{ host: 'gw.example.com', port: 22, username: 'j', auth_method: { type: 'Agent' } }];
const s = {
	host: 'server.example.com',
	port: 2222,
	host_label: 'Internet',
	jump_chain: jump,
	alt_addresses: [
		{ label: 'Office', host: '10.0.0.5', port: 22, direct: true },
		{ host: '2001:db8::5', direct: true },
		{ label: 'Blank', host: '  ' },
	],
};
const all = sessionAddresses(s);
check('blank addresses are left out', all.length === 3, JSON.stringify(all));
check('main first, through the jump host', all[0].host === 'server.example.com' && all[0].routed && all[0].label === 'Internet');
check('a direct address skips the route', !all[1].routed && all[1].port === 22);
check("an address without a port takes the session's", all[2].port === 2222);
const viaS = { host: '10.0.0.5', port: 22, alt_addresses: [{ label: 'Branch', host: '10.0.1.5', via_session_id: 'gw', direct: true }] };
const viaAll = sessionAddresses(viaS);
check('an address through a saved session is routed, not direct', viaAll[1].routed && viaAll[1].viaSessionId === 'gw');
check('a session without a route keeps its main address direct', !viaAll[0].routed && !viaAll[0].viaSessionId);
// WireGuard: the main address carries the session's tunnel (and keeps
// its jump hosts, the tunnel carrying the first); another address can go
// through its own tunnel alone. Neither can be checked from here.
const wgS = { host: '10.8.0.5', port: 22, wireguard_id: 'office-wg', alt_addresses: [{ label: 'Lab', host: '10.9.0.5', via_wireguard: 'lab-wg', via_session_id: 'gw' }, { host: '192.168.1.5', direct: true }] };
const wgAll = sessionAddresses(wgS);
check('main address goes through the session tunnel', wgAll[0].wireguardId === 'office-wg' && wgAll[0].routed && !wgAll[0].tunnelOnly);
check('an address through its own tunnel, alone', wgAll[1].wireguardId === 'lab-wg' && wgAll[1].tunnelOnly && wgAll[1].routed && !wgAll[1].viaSessionId);
check('a direct address has no tunnel', !wgAll[2].wireguardId && !wgAll[2].routed);
check('tunnelled addresses are not checked first', autoOrder(wgAll, [5, 5, null]).map((a) => a.index).join() === '0,1,2');
check('mode defaults to auto', connectMode(s) === 'auto');
check('ask is kept', connectMode({ ...s, connect_mode: 'ask' }) === 'ask');
check('other kinds ignore other addresses', !hasAltAddresses({ ...s, kind: 'rdp' }));
check('IPv6 keys are bracketed', addressKey(all[2]) === '[2001:db8::5]:2222');

// The office answers: it goes first, then the routed main, then the silent one.
let order = autoOrder(all, [undefined, 3, null]);
check('answering direct first', order.map((a) => a.index).join() === '1,0,2', order.map((a) => a.index).join());
// Away from the office: the routed main first.
order = autoOrder(all, [undefined, null, null]);
check('routed before the silent ones', order.map((a) => a.index).join() === '0,1,2', order.map((a) => a.index).join());
// Both direct ones answer, IPv6 worked last time.
order = autoOrder(all, [undefined, 3, 4], '[2001:db8::5]:2222');
check('last good goes first within its group', order.map((a) => a.index).join() === '2,1,0', order.map((a) => a.index).join());
// The last good one is routed while the office answers: the office still wins.
order = autoOrder(all, [undefined, 3, null], 'server.example.com:2222');
check('last good never beats an answering direct one', order[0].index === 1);

// Silent mode: in priority order, the last good one first.
check('silent keeps priority order', plainOrder(all).map((a) => a.index).join() === '0,1,2');
check('silent puts the last good first', plainOrder(all, '10.0.0.5:22').map((a) => a.index).join() === '1,0,2');
check('the session wins over the setting', checksAddresses({ address_check: false }, true) === false && checksAddresses({ address_check: true }, false) === true);
check('unset follows the setting', checksAddresses({}, false) === false && checksAddresses({ address_check: null }, true) === true);

check('a timeout moves on', isUnreachable('Connection failed: Connection timed out'));
check('a refused port moves on', isUnreachable('Connection failed: could not reach 10.0.0.5:22: connection refused'));
check('a refused login does not', !isUnreachable('Authentication rejected — server did not accept the public key'));
check('a key error does not', !isUnreachable('Connection failed: Auth error: bad passphrase'));
check('an unknown server key turned down does not', !isUnreachable('Connection failed: Unknown server key'));
check('a name that does not resolve moves on', isUnreachable('Connection failed: failed to lookup address information: Name or service not known'));
check('a rejected host key does not', !isUnreachable('Connection failed: Host key rejected by user'));

console.log('Tooltip text');
{
	const s = { ...plain, alt_addresses: [{ label: 'Office', host: '10.0.0.5' }, { label: '', host: '10.0.0.6', port: 2222 }] };
	const [, office, bare] = sessionAddresses(s);
	check('label with its address', addressLabelled(office, 22) === 'Office (10.0.0.5)');
	check('no label: the address alone, with its own port', addressLabelled(bare, 22) === '10.0.0.6:2222');
}

if (failures) {
	console.log(`\n${failures} failed`);
	process.exit(1);
}
console.log('\nall passed');
