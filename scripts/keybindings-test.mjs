/**
 * Tests the rules of configurable keyboard shortcuts (#lib/state/keybindings):
 * reading a key press, saving the user's keys, conflicts and reset.
 *
 * Run: node --import ./scripts/resolve-lib.mjs scripts/keybindings-test.mjs
 */

import {
	ACTIONS, parseChord, formatChord, canonical, chordFromEvent, keysFor, defaultKeys, isDefault,
	keyMap, conflictFor, conflicts, rejectReason, assignKey, replaceKey, removeKey, resetAction,
	readOverrides, displayChord
} from '#lib/state/keybindings.js';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name}${extra ? '\n        ' + extra : ''}`);
	}
};
const section = (s) => console.log(`\n${s}`);
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const ev = (key, code, mods = {}) => ({
	key, code, ctrlKey: !!mods.ctrl, metaKey: !!mods.meta, altKey: !!mods.alt, shiftKey: !!mods.shift
});

section('chords');
check('parse Mod+Shift+T', same(parseChord('Mod+Shift+T'), { mod: true, ctrl: false, alt: false, shift: true, key: 'T' }));
check('parse Mod++', parseChord('Mod++')?.key === '+' && parseChord('Mod++')?.mod === true);
check('parse a bare key', parseChord('F5')?.key === 'F5');
check('reject an unknown modifier', parseChord('Hyper+T') === null);
check('reject empty', parseChord('') === null && parseChord('Mod+') === null);
check('format round trip', formatChord(parseChord('Mod+Ctrl+Alt+Shift+K')) === 'Mod+Ctrl+Alt+Shift+K');
check('Ctrl is Mod off a Mac', canonical('Ctrl+T', false) === 'Mod+T');
check('Ctrl stays Ctrl on a Mac', canonical('Ctrl+T', true) === 'Ctrl+T');

section('key presses');
check('Ctrl+T on Linux', chordFromEvent(ev('t', 'KeyT', { ctrl: true }), false) === 'Mod+T');
check('Cmd+T on a Mac', chordFromEvent(ev('t', 'KeyT', { meta: true }), true) === 'Mod+T');
check('Control+Tab on a Mac', chordFromEvent(ev('Tab', 'Tab', { ctrl: true }), true) === 'Ctrl+Tab');
check('Russian layout: Ctrl+е is Ctrl+T', chordFromEvent(ev('е', 'KeyT', { ctrl: true }), false) === 'Mod+T');
check('Dvorak: the letter typed, not the place', chordFromEvent(ev('k', 'KeyT', { ctrl: true }), false) === 'Mod+K');
check('Ctrl+Shift+1 is not Ctrl+Shift+!', chordFromEvent(ev('!', 'Digit1', { ctrl: true, shift: true }), false) === 'Mod+Shift+1');
check('Ctrl+Shift+= is Ctrl+Shift++', chordFromEvent(ev('+', 'Equal', { ctrl: true, shift: true }), false) === 'Mod+Shift++');
check('number pad +', chordFromEvent(ev('+', 'NumpadAdd', { ctrl: true }), false) === 'Mod++');
check('Ctrl+,', chordFromEvent(ev(',', 'Comma', { ctrl: true }), false) === 'Mod+,');
check('a modifier alone is nothing', chordFromEvent(ev('Control', 'ControlLeft', { ctrl: true }), false) === null);
check('the Windows key is the system\'s', chordFromEvent(ev('t', 'KeyT', { meta: true }), false) === null);
check('Space has a name', chordFromEvent(ev(' ', 'Space', { ctrl: true }), false) === 'Mod+Space');

section('defaults');
check('every action has a default key', ACTIONS.every((a) => a.defaults.length > 0));
check('no two actions share a default (PC)', conflicts({}, false).size === 0, JSON.stringify([...conflicts({}, false)]));
check('no two actions share a default (Mac)', conflicts({}, true).size === 0, JSON.stringify([...conflicts({}, true)]));
check('every default needs a modifier', ACTIONS.every((a) => [...a.defaults, ...(a.macDefaults ?? [])].every((k) => rejectReason(k) === null)));
check('next tab on a Mac is Control+Tab or Cmd+Alt+Right', same(defaultKeys('tab.next', true), ['Ctrl+Tab', 'Mod+Alt+ArrowRight']));
check('Ctrl+1…8 go to tabs 1…8', [1, 2, 3, 4, 5, 6, 7, 8].every((n) => keyMap({}, false).get(`Mod+${n}`) === `tab.go${n}`));
check('Ctrl+9 goes to the last tab', keyMap({}, false).get('Mod+9') === 'tab.goLast');
check('Cmd+1 on a Mac', chordFromEvent(ev('1', 'Digit1', { meta: true }), true) === 'Mod+1' && keyMap({}, true).get('Mod+1') === 'tab.go1');
check('Ctrl+1 on a Czech layout (types +)', chordFromEvent(ev('+', 'Digit1', { ctrl: true }), false) === 'Mod+1');
check('previously used tab', keyMap({}, false).get('Mod+Shift+L') === 'tab.last');
check('no Alt+digit taken from shells', ![...keyMap({}, false).keys()].some((k) => /^Alt\+[0-9]$/.test(k)));
check('Ctrl+PgUp/PgDn left to the terminal', !keyMap({}, false).has('Mod+PageUp') && !keyMap({}, false).has('Mod+PageDown'));
check('key map finds new tab', keyMap({}, false).get('Mod+T') === 'tab.new');
check('zoom in has three keys', keysFor('zoom.in', {}, false).length === 3);

section('changing keys');
let o = assignKey({}, 'tab.new', 'Mod+Alt+T', false);
check('a second key is added', same(keysFor('tab.new', o, false), ['Mod+T', 'Mod+Alt+T']));
check('no longer default', !isDefault('tab.new', o, false));
o = replaceKey({}, 'tab.new', 'Mod+T', 'Mod+Alt+T', false);
check('replace swaps one key', same(keysFor('tab.new', o, false), ['Mod+Alt+T']));
check('the old key starts nothing', keyMap(o, false).get('Mod+T') === undefined);
check('the new key starts it', keyMap(o, false).get('Mod+Alt+T') === 'tab.new');
check('conflict found', conflictFor('Mod+Shift+P', o, false, 'tab.new') === 'app.palette');
check('own key is no conflict', conflictFor('Mod+Alt+T', o, false, 'tab.new') === null);
check('Ctrl and Mod are the same key off a Mac', conflictFor('Ctrl+Shift+P', {}, false) === 'app.palette');
o = assignKey(o, 'tab.new', 'Mod+Shift+P', false);
check('taking a key moves it', keysFor('tab.new', o, false).includes('Mod+Shift+P') && keysFor('app.palette', o, false).length === 0);
check('still no shared keys', conflicts(o, false).size === 0);
o = removeKey({}, 'app.aiPanel', 'Mod+Shift+A', false);
check('removing the last key leaves none', same(o, { 'app.aiPanel': [] }));
check('an action with no key is not in the map', ![...keyMap(o, false).values()].includes('app.aiPanel'));
o = resetAction(o, 'app.aiPanel', false);
check('reset brings the default back', same(o, {}) && isDefault('app.aiPanel', o, false));
o = replaceKey({}, 'tab.new', 'Mod+T', 'Mod+T', false);
check('setting a default back stores nothing', same(o, {}));

section('what may be a shortcut');
check('a plain letter may not', rejectReason('T') === 'needs_modifier');
check('Shift+letter may not', rejectReason('Shift+T') === 'needs_modifier');
check('a function key may', rejectReason('F2') === null);
check('Alt+letter may', rejectReason('Alt+T') === null);

section('stored copy');
check('unknown actions and junk dropped', same(readOverrides({ x: ['Mod+T'], 'tab.new': ['Mod+Q', 'Hyper+Z', 7] }), { 'tab.new': ['Mod+Q'] }));
check('not an object', same(readOverrides(null), {}) && same(readOverrides(['a']), {}));

section('written for people');
check('PC', displayChord('Mod+Shift+T', false) === 'Ctrl+Shift+T');
check('Mac', displayChord('Mod+Shift+T', true) === '⇧⌘T');
check('Mac Control', displayChord('Ctrl+Tab', true) === '⌃⇥');
check('PC arrows', displayChord('Mod+ArrowUp', false) === 'Ctrl+Up');
check('plus', displayChord('Mod++', false) === 'Ctrl++');

console.log(failures ? `\n${failures} failed` : '\nall passed');
process.exit(failures ? 1 : 0);
