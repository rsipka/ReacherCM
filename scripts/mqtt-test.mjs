/**
 * The MQTT explorer's topic tree and payload views (#lib/mqtt).
 * Run: node --import ./scripts/resolve-lib.mjs scripts/mqtt-test.mjs
 */
import { TopicTree, visibleRows, validFilter, validTopic, HISTORY } from '#lib/mqtt/topics.js';
import { asJson, autoView, encode, hexDump, preview, render } from '#lib/mqtt/payload.js';

let failures = 0;
const check = (name, cond) => {
	console.log(`  ${cond ? 'PASS' : 'FAIL'}  ${name}`);
	if (!cond) failures++;
};
const msg = (topic, payload = '', retain = false) => ({ topic, payload: btoa(payload), size: payload.length, qos: 0, retain, time: 0 });
const bytes = (s) => new TextEncoder().encode(s);

const tree = new TopicTree();
tree.add([msg('home/kitchen/temp', '21'), msg('home/kitchen/temp', '22'), msg('home/hall/light', 'on'), msg('$SYS/broker/uptime', '5')]);
check('counts below a level', tree.find('home').total === 3 && tree.find('home').topics === 2);
check('last message kept', atob(tree.find('home/kitchen/temp').last.payload) === '22');
check('history newest first', tree.find('home/kitchen/temp').history.length === 2 && atob(tree.find('home/kitchen/temp').history[0].payload) === '22');
for (let i = 0; i < HISTORY + 10; i++) tree.add([msg('a', String(i))]);
check('history capped', tree.find('a').history.length === HISTORY && tree.find('a').count === HISTORY + 10);

const closed = visibleRows(tree, new Set());
check('closed tree shows the top levels, sorted', closed.map((r) => r.node.name).join(',') === '$SYS,a,home');
const opened = visibleRows(tree, new Set(['home']));
check('an open level shows its children', opened.map((r) => r.node.path).join(',') === '$SYS,a,home,home/hall,home/kitchen');
const filtered = visibleRows(tree, new Set(), 'TEMP');
check('a filter opens the way to matches', filtered.map((r) => r.node.path).join(',') === 'home,home/kitchen,home/kitchen/temp');

check('filters', validFilter('a/+/#') && !validFilter('a/#/b') && !validFilter('a+') && validFilter('#'));
check('topics', validTopic('a/b') && !validTopic('a/#') && !validTopic(''));

check('JSON is indented', asJson('{"a":1}') === '{\n  "a": 1\n}');
check('a bare number is text', autoView(bytes('42')) === 'text');
check('JSON auto', autoView(bytes(' [1,2] ')) === 'json');
check('binary auto is hex', autoView(new Uint8Array([0xff, 0x00, 0xfe])) === 'hex');
check('hex dump', hexDump(bytes('AB')).startsWith('00000000  41 42') && hexDump(bytes('AB')).endsWith('AB'));
check('render text', render(bytes('hi'), 'text').body === 'hi');
check('preview squashes lines', preview(bytes('a\n  b')) === 'a b');
check('encode hex', encode('0a FF', 'hex').join(',') === '10,255');
let threw = false;
try {
	encode('abc', 'hex');
} catch {
	threw = true;
}
check('odd hex refused', threw);

if (failures) {
	console.log(`\n${failures} failed`);
	process.exit(1);
}
console.log('\nall passed');
