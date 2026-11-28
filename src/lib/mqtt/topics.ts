/**
 * An MQTT session's topic tree: every topic seen, by level, with its last
 * message, a short history and how many messages arrived under it. Kept
 * out of Svelte's reach (a busy broker sends thousands a second); the
 * panel redraws on `version`.
 */
import type { MqttMessage } from '#lib/ipc/mqtt.js';

/** Messages kept per topic. */
export const HISTORY = 50;
/** Topics kept at most; past this, new ones are counted, not kept. */
export const MAX_TOPICS = 20_000;

export interface TopicNode {
	/** This level's name ('' is a level too: `a//b`). */
	name: string;
	/** The full topic. */
	path: string;
	children: Map<string, TopicNode>;
	/** The last message on this exact topic. */
	last?: MqttMessage;
	/** Newest first. */
	history: MqttMessage[];
	/** Messages on this exact topic. */
	count: number;
	/** Messages here and below. */
	total: number;
	/** Topics here and below that have had a message. */
	topics: number;
}

function node(name: string, path: string): TopicNode {
	return { name, path, children: new Map(), history: [], count: 0, total: 0, topics: 0 };
}

export class TopicTree {
	root: TopicNode = node('', '');
	/** Bumped on every change. */
	version = 0;
	/** Messages dropped because the tree was full. */
	dropped = 0;
	private size = 0;

	clear(): void {
		this.root = node('', '');
		this.size = 0;
		this.dropped = 0;
		this.version++;
	}

	add(messages: MqttMessage[]): void {
		for (const m of messages) this.addOne(m);
		this.version++;
	}

	private addOne(m: MqttMessage): void {
		const levels = m.topic.split('/');
		const trail: TopicNode[] = [this.root];
		let at = this.root;
		for (let i = 0; i < levels.length; i++) {
			let next = at.children.get(levels[i]);
			if (!next) {
				if (this.size >= MAX_TOPICS) {
					this.dropped++;
					return;
				}
				next = node(levels[i], levels.slice(0, i + 1).join('/'));
				at.children.set(levels[i], next);
				this.size++;
			}
			at = next;
			trail.push(at);
		}
		const isNew = at.count === 0;
		// An empty retained message clears the topic's retained one.
		at.last = m;
		at.count++;
		at.history.unshift(m);
		if (at.history.length > HISTORY) at.history.length = HISTORY;
		for (const n of trail) {
			n.total++;
			if (isNew) n.topics++;
		}
	}

	find(path: string): TopicNode | undefined {
		let at: TopicNode | undefined = this.root;
		for (const level of path.split('/')) {
			at = at?.children.get(level);
			if (!at) return undefined;
		}
		return at;
	}
}

export interface Row {
	node: TopicNode;
	depth: number;
	open: boolean;
	/** Has children to open. */
	parent: boolean;
}

function sorted(n: TopicNode): TopicNode[] {
	return [...n.children.values()].sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
}

/** Whether a topic matches a filter typed in: its words, in any case. */
export function topicMatches(path: string, filter: string): boolean {
	const p = path.toLowerCase();
	return filter
		.toLowerCase()
		.split(/\s+/)
		.filter(Boolean)
		.every((w) => p.includes(w));
}

/**
 * The tree's rows as shown: the children of open nodes. With a filter,
 * the topics that match it and the levels above them, all open.
 */
export function visibleRows(tree: TopicTree, open: Set<string>, filter = ''): Row[] {
	const rows: Row[] = [];
	const f = filter.trim();
	const keep = (n: TopicNode): boolean => (n.count > 0 && topicMatches(n.path, f)) || [...n.children.values()].some(keep);
	const walk = (n: TopicNode, depth: number) => {
		for (const c of sorted(n)) {
			if (f && !keep(c)) continue;
			const isOpen = f ? true : open.has(c.path);
			rows.push({ node: c, depth, open: isOpen, parent: c.children.size > 0 });
			if (isOpen) walk(c, depth + 1);
		}
	};
	walk(tree.root, 0);
	return rows;
}

/** Whether a topic filter is well formed (`#` last and alone, `+` alone). */
export function validFilter(filter: string): boolean {
	if (!filter) return false;
	const levels = filter.split('/');
	return levels.every((l, i) => (l === '#' ? i === levels.length - 1 : l === '+' || !/[#+]/.test(l)));
}

/** Whether a topic can be published to. */
export function validTopic(topic: string): boolean {
	return !!topic && !/[#+]/.test(topic);
}
