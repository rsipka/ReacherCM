/**
 * How an MQTT payload is shown: as text, as indented JSON when it is
 * JSON, or as a hex dump; and how a payload typed in is turned to bytes.
 */

export type PayloadView = 'auto' | 'text' | 'json' | 'hex';

/** The payload as text, or null when it is not UTF-8. */
export function asText(bytes: Uint8Array): string | null {
	try {
		return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
	} catch {
		return null;
	}
}

/** Indented JSON, or null when the text is not JSON (a bare number or
 *  string counts as text, not JSON). */
export function asJson(text: string | null): string | null {
	if (text === null) return null;
	const t = text.trim();
	if (!t.startsWith('{') && !t.startsWith('[')) return null;
	try {
		return JSON.stringify(JSON.parse(t), null, 2);
	} catch {
		return null;
	}
}

/** `offset  hex bytes  ascii`, 16 bytes a line. */
export function hexDump(bytes: Uint8Array, max = 64 * 1024): string {
	const lines: string[] = [];
	const n = Math.min(bytes.length, max);
	for (let i = 0; i < n; i += 16) {
		const row = bytes.subarray(i, Math.min(i + 16, n));
		const hex = [...row].map((b) => b.toString(16).padStart(2, '0')).join(' ');
		const ascii = [...row].map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : '.')).join('');
		lines.push(`${i.toString(16).padStart(8, '0')}  ${hex.padEnd(47)}  ${ascii}`);
	}
	if (bytes.length > max) lines.push('…');
	return lines.join('\n');
}

/** What `auto` shows: JSON, else text, else hex. */
export function autoView(bytes: Uint8Array): Exclude<PayloadView, 'auto'> {
	const text = asText(bytes);
	if (asJson(text) !== null) return 'json';
	if (text !== null) return 'text';
	return 'hex';
}

/** The payload as the chosen view shows it. */
export function render(bytes: Uint8Array, view: PayloadView): { view: Exclude<PayloadView, 'auto'>; body: string } {
	const v = view === 'auto' ? autoView(bytes) : view;
	const text = asText(bytes);
	if (v === 'json') return { view: v, body: asJson(text) ?? text ?? hexDump(bytes) };
	if (v === 'text') return { view: v, body: text ?? new TextDecoder().decode(bytes) };
	return { view: v, body: hexDump(bytes) };
}

/** A short one-line preview for the tree. */
export function preview(bytes: Uint8Array, max = 80): string {
	if (bytes.length === 0) return '';
	const text = asText(bytes);
	if (text === null) return [...bytes.subarray(0, 24)].map((b) => b.toString(16).padStart(2, '0')).join(' ') + (bytes.length > 24 ? ' …' : '');
	const line = text.replace(/\s+/g, ' ').trim();
	return line.length > max ? line.slice(0, max) + '…' : line;
}

/** Bytes from what was typed: text as UTF-8, or hex (`0a ff`, `0aff`). */
export function encode(input: string, as: 'text' | 'hex'): Uint8Array {
	if (as === 'text') return new TextEncoder().encode(input);
	const clean = input.replace(/0x/gi, '').replace(/[\s,:]/g, '');
	if (clean.length % 2 !== 0 || /[^0-9a-f]/i.test(clean)) throw new Error('hex');
	const out = new Uint8Array(clean.length / 2);
	for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
	return out;
}

/** A size as people read it. */
export function size(n: number): string {
	if (n < 1024) return `${n} B`;
	if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
	return `${(n / 1024 / 1024).toFixed(1)} MiB`;
}
