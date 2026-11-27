/**
 * Notifications (toasts): where they go, and what the history keeps
 * (#lib/state/notify-rules).
 * Run: node --import ./scripts/resolve-lib.mjs scripts/notify-test.mjs
 */
import {
	routeToast,
	toastPosition,
	systemNotifyMode,
	pruneHistory,
	parseHistory,
	unreadCount,
	HISTORY_KEEP_MS,
	HISTORY_MAX
} from '#lib/state/notify-rules.js';

let failures = 0;
const check = (name, cond) => {
	console.log(`  ${cond ? 'PASS' : 'FAIL'}  ${name}`);
	if (!cond) failures++;
};
const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const now = 10 * HISTORY_KEEP_MS;
const entry = (id, at, type = 'info') => ({ id, message: `m${id}`, type, at });

check('off: in the app only, in front or not', eq(routeToast('off', false, false), { inApp: true, system: false }));
check('background, app in front: in the app only', eq(routeToast('background', true, false), { inApp: true, system: false }));
check('background, app behind: both', eq(routeToast('background', false, false), { inApp: true, system: true }));
check('always: the system instead of the app', eq(routeToast('always', true, false), { inApp: false, system: true }));
check('always, with a button: the app keeps it', eq(routeToast('always', true, true), { inApp: true, system: true }));

check('a saved position is kept', toastPosition('bottom-right') === 'bottom-right');
check('an unknown position falls back to top centre', toastPosition('middle') === 'top-center' && toastPosition(undefined) === 'top-center');
check('an unknown mode falls back to off', systemNotifyMode('sometimes') === 'off' && systemNotifyMode('always') === 'always');

check('a day-old notification is dropped', eq(pruneHistory([entry('a', now - HISTORY_KEEP_MS), entry('b', now - 1000)], now).map((e) => e.id), ['b']));
check('kept oldest first', eq(pruneHistory([entry('b', now - 10), entry('a', now - 20)], now).map((e) => e.id), ['a', 'b']));
const many = Array.from({ length: HISTORY_MAX + 5 }, (_, i) => entry(String(i), now - 1000 + i));
const capped = pruneHistory(many, now);
check('at most HISTORY_MAX, the newest kept', capped.length === HISTORY_MAX && capped[capped.length - 1].id === String(HISTORY_MAX + 4) && capped[0].id === '5');

check('garbage reads as empty', parseHistory('not json', now).length === 0 && parseHistory('{"a":1}', now).length === 0 && parseHistory(null, now).length === 0);
const raw = JSON.stringify([entry('a', now - 5, 'error'), { id: 1 }, null, { id: 'c', message: 'x', at: now - 1, type: 'odd' }]);
const parsed = parseHistory(raw, now);
check('malformed entries are skipped, an odd type reads as info', eq(parsed.map((e) => [e.id, e.type]), [['a', 'error'], ['c', 'info']]));

check('unread: only those after the last look', unreadCount([entry('a', 10), entry('b', 20), entry('c', 30)], 20) === 1);

console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
