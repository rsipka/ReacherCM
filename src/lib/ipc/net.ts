import { Channel, invoke } from '@tauri-apps/api/core';

/** Network tools (src-tauri/src/net): ping, port scan, Wake-on-LAN. */

export type PingMethod = 'auto' | 'icmp' | 'tcp';

export interface PingReply {
  ok: boolean;
  ms: number | null;
  /** What was sent: ICMP echo, or a TCP connect when ICMP is not allowed. */
  method: 'icmp' | 'tcp';
  ttl: number | null;
  error: string | null;
}

export type ScanEvent = { type: 'open'; port: number; ms: number } | { type: 'progress'; done: number; total: number };

export interface ScanSummary {
  open: number[];
  closed: number;
  filtered: number;
  scanned: number;
  cancelled: boolean;
  ms: number;
}

/** The addresses a name resolves to, IPv4 first. */
export function netResolve(host: string): Promise<string[]> {
  return invoke<string[]>('net_resolve', { host });
}

/** One ping of an IP address. `port` is where a TCP ping goes. */
export function netPing(address: string, opts: { method?: PingMethod; port?: number; timeoutMs?: number; seq?: number } = {}): Promise<PingReply> {
  return invoke<PingReply>('net_ping', {
    address,
    method: opts.method ?? 'auto',
    port: opts.port ?? null,
    timeoutMs: opts.timeoutMs ?? null,
    seq: opts.seq ?? null,
  });
}

/** Scan ports of an IP address; `onEvent` hears each open port and the progress. */
export function netScanPorts(
  id: string,
  address: string,
  ports: number[],
  opts: { timeoutMs?: number; concurrency?: number },
  onEvent: (e: ScanEvent) => void,
): Promise<ScanSummary> {
  const channel = new Channel<ScanEvent>();
  channel.onmessage = onEvent;
  return invoke<ScanSummary>('net_scan_ports', {
    id,
    address,
    ports,
    timeoutMs: opts.timeoutMs ?? null,
    concurrency: opts.concurrency ?? null,
    onEvent: channel,
  });
}

export function netScanCancel(id: string): Promise<void> {
  return invoke('net_scan_cancel', { id });
}

/** Send a Wake-on-LAN magic packet. */
export function netWake(mac: string, broadcast?: string | null, port?: number | null): Promise<void> {
  return invoke('net_wake', { mac, broadcast: broadcast?.trim() || null, port: port || null });
}
