/**
 * A picture of what a terminal shows now: its visible rows drawn from the
 * buffer onto a canvas, in the terminal's own font and colours. Drawn here
 * rather than copied from the screen, so it works the same with the WebGL
 * and the DOM renderer and has none of the app around it (no selection, no
 * find marks, no cursor).
 */
import type { IBufferCell, ITheme, Terminal } from '@xterm/xterm';

/** The screenshot button's icon (24×24, stroked). */
export const CAMERA_ICON = 'M4 8h3l2-3h6l2 3h3a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1zM12 10.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7z';

const ANSI_KEYS: (keyof ITheme)[] = [
	'black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
	'brightBlack', 'brightRed', 'brightGreen', 'brightYellow', 'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite'
];

/** xterm's own colours, for a theme that leaves some out. */
const DEFAULT_ANSI = [
	'#2e3436', '#cc0000', '#4e9a06', '#c4a000', '#3465a4', '#75507b', '#06989a', '#d3d7cf',
	'#555753', '#ef2929', '#8ae234', '#fce94f', '#729fcf', '#ad7fa8', '#34e2e2', '#eeeeec'
];

function hex(n: number): string {
	return n.toString(16).padStart(2, '0');
}

/** The 256 colours: the theme's 16, then the 6×6×6 cube and the greys. */
export function palette(theme: ITheme = {}): string[] {
	const colors = ANSI_KEYS.map((k, i) => (theme[k] as string | undefined) ?? DEFAULT_ANSI[i]);
	const steps = [0, 95, 135, 175, 215, 255];
	for (let i = 0; i < 216; i++) {
		const r = steps[Math.floor(i / 36) % 6];
		const g = steps[Math.floor(i / 6) % 6];
		const b = steps[i % 6];
		colors.push(`#${hex(r)}${hex(g)}${hex(b)}`);
	}
	for (let i = 0; i < 24; i++) {
		const v = 8 + i * 10;
		colors.push(`#${hex(v)}${hex(v)}${hex(v)}`);
	}
	theme.extendedAnsi?.forEach((c, i) => {
		if (c) colors[16 + i] = c;
	});
	return colors;
}

function rgb(n: number): string {
	return `#${hex((n >> 16) & 255)}${hex((n >> 8) & 255)}${hex(n & 255)}`;
}

export interface SnapshotOptions {
	/** Pixels per CSS pixel; at least 2 so text stays sharp. */
	scale?: number;
	/** Room around the text, in CSS pixels. */
	padding?: number;
}

/** The terminal's visible rows as a canvas. `screen` is its `.xterm-screen`
 *  element, which gives the size of a character cell. */
export function snapshotTerminal(term: Terminal, screen: HTMLElement | null, opts: SnapshotOptions = {}): HTMLCanvasElement {
	const cols = term.cols;
	const rows = term.rows;
	const o = term.options;
	const fontSize = o.fontSize ?? 14;
	const fontFamily = o.fontFamily ?? 'monospace';
	const rect = screen?.getBoundingClientRect();
	const cellW = rect && rect.width > 0 ? rect.width / cols : fontSize * 0.6;
	const cellH = rect && rect.height > 0 ? rect.height / rows : fontSize * (o.lineHeight ?? 1) * 1.2;
	const pad = opts.padding ?? 8;
	const scale = opts.scale ?? Math.max(2, window.devicePixelRatio || 1);

	const theme = o.theme ?? {};
	const colors = palette(theme);
	const fgDefault = theme.foreground ?? '#ffffff';
	const bgDefault = theme.background ?? '#000000';
	const boldBright = o.drawBoldTextInBrightColors ?? true;
	const weight = String(o.fontWeight ?? 'normal');
	const weightBold = String(o.fontWeightBold ?? 'bold');

	const canvas = document.createElement('canvas');
	canvas.width = Math.ceil((cols * cellW + pad * 2) * scale);
	canvas.height = Math.ceil((rows * cellH + pad * 2) * scale);
	const ctx = canvas.getContext('2d');
	if (!ctx) throw new Error('No canvas');
	ctx.scale(scale, scale);
	ctx.fillStyle = bgDefault;
	ctx.fillRect(0, 0, cols * cellW + pad * 2, rows * cellH + pad * 2);
	ctx.textBaseline = 'middle';

	const buffer = term.buffer.active;
	let cell: IBufferCell | undefined;

	const fgOf = (c: IBufferCell): string => {
		if (c.isFgRGB()) return rgb(c.getFgColor());
		if (c.isFgPalette()) {
			let n = c.getFgColor();
			if (boldBright && c.isBold() && n < 8) n += 8;
			return colors[n] ?? fgDefault;
		}
		return fgDefault;
	};
	const bgOf = (c: IBufferCell): string | null => {
		if (c.isBgRGB()) return rgb(c.getBgColor());
		if (c.isBgPalette()) return colors[c.getBgColor()] ?? null;
		return null;
	};

	for (let y = 0; y < rows; y++) {
		const line = buffer.getLine(buffer.viewportY + y);
		if (!line) continue;
		const top = pad + y * cellH;
		for (let x = 0; x < cols; x++) {
			cell = line.getCell(x, cell);
			if (!cell) continue;
			const width = cell.getWidth();
			if (width === 0) continue; // the second half of a wide character
			let fg = fgOf(cell);
			let bg = bgOf(cell);
			if (cell.isInverse()) {
				const swap = bg ?? bgDefault;
				bg = fg;
				fg = swap;
			}
			const left = pad + x * cellW;
			const w = cellW * width;
			if (bg) {
				ctx.fillStyle = bg;
				// A hair wider, so neighbouring cells leave no seams.
				ctx.fillRect(left, top, w + 0.5, cellH + 0.5);
			}
			const chars = cell.getChars();
			if (!chars || chars === ' ' || cell.isInvisible()) continue;
			ctx.globalAlpha = cell.isDim() ? 0.5 : 1;
			ctx.fillStyle = fg;
			ctx.strokeStyle = fg;
			ctx.font = `${cell.isItalic() ? 'italic ' : ''}${cell.isBold() ? weightBold : weight} ${fontSize}px ${fontFamily}`;
			ctx.fillText(chars, left, top + cellH / 2, w + cellW * 0.25);
			if (cell.isUnderline()) ctx.fillRect(left, top + cellH - Math.max(1, cellH * 0.08), w, 1);
			if (cell.isStrikethrough()) ctx.fillRect(left, top + cellH / 2, w, 1);
			if (cell.isOverline()) ctx.fillRect(left, top, w, 1);
			ctx.globalAlpha = 1;
		}
	}
	return canvas;
}

/** A file name from the session's name and the time: `web-01 2026-11-23 21-05-09.png`. */
export function screenshotName(name: string, now = new Date()): string {
	const p = (n: number) => String(n).padStart(2, '0');
	const stamp = `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())} ${p(now.getHours())}-${p(now.getMinutes())}-${p(now.getSeconds())}`;
	// eslint-disable-next-line no-control-regex
	const safe = name.replace(/[\u0000-\u001f<>:"/\\|?*]+/g, '_').replace(/^[.\s]+|[.\s]+$/g, '').slice(0, 80);
	return `${safe || 'screenshot'} ${stamp}.png`;
}
