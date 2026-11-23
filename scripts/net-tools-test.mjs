/**
 * Tests the network tools' pure logic (#lib/net/tools): port lists, MAC
 * addresses and ping statistics.
 *
 * Run: node --import ./scripts/resolve-lib.mjs scripts/net-tools-test.mjs
 */

import { COMMON_PORTS, formatMac, formatMs, parseMac, parsePorts, pingStats, presetPorts, serviceName } from '../src/lib/net/tools.ts';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name} ${extra}`);
	}
};
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

check('ports and ranges, sorted, each once', same(parsePorts('443, 22 80;8000-8002 22').ports, [22, 80, 443, 8000, 8001, 8002]));
check('a reversed range is read the right way round', same(parsePorts('25-23').ports, [23, 24, 25]));
check('an empty list is no error', same(parsePorts('  ').ports, []) && parsePorts('').error === null);
check('a word is an error', parsePorts('22, ssh').error === 'ssh');
check('port 0 and 65536 are errors', parsePorts('0').error === '0' && parsePorts('1-65536').error === '1-65536');
check('common ports are sorted and include SSH, RDP and HTTPS', COMMON_PORTS.includes(22) && COMMON_PORTS.includes(3389) && COMMON_PORTS.includes(443) && same(COMMON_PORTS, [...COMMON_PORTS].sort((a, b) => a - b)));
check('presets', presetPorts('well-known').length === 1024 && presetPorts('all').length === 65535 && same(presetPorts('custom', '1-3'), [1, 2, 3]));
check('service names', serviceName(22) === 'SSH' && serviceName(1) === '');

check('MAC spellings', same(parseMac('00-11-22-aa-bb-cc'), parseMac('0011.22AA.BBCC')) && parseMac('001122aabbcc')?.[5] === 0xcc);
check('not a MAC', parseMac('00:11:22:aa:bb') === null && parseMac('gg:11:22:aa:bb:cc') === null);
check('MAC formatted', formatMac('0011.22aa.bbcc') === '00:11:22:AA:BB:CC' && formatMac('nope') === 'nope');

const st = pingStats([10, null, 20, 30]);
check('ping stats', st.sent === 4 && st.received === 3 && st.loss === 25 && st.min === 10 && st.max === 30 && st.avg === 20);
const none = pingStats([null, null]);
check('all lost', none.loss === 100 && none.avg === null);
check('no pings', pingStats([]).loss === 0);
check('round trips formatted', formatMs(0.42) === '0.4 ms' && formatMs(12.6) === '13 ms' && formatMs(null) === '–');

if (failures) {
	console.log(`\n${failures} failed`);
	process.exit(1);
}
console.log('\nall passed');
