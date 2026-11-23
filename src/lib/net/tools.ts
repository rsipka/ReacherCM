/**
 * Network tools' pure logic: port lists, service names, MAC addresses and
 * ping statistics. No IPC, so scripts/net-tools-test.mjs can run it.
 */

/** Well-known TCP ports and what usually listens there. */
export const SERVICES: Record<number, string> = {
  20: 'FTP data', 21: 'FTP', 22: 'SSH', 23: 'Telnet', 25: 'SMTP', 53: 'DNS', 80: 'HTTP', 88: 'Kerberos',
  110: 'POP3', 111: 'RPC', 135: 'MS RPC', 139: 'NetBIOS', 143: 'IMAP', 161: 'SNMP', 179: 'BGP', 389: 'LDAP',
  443: 'HTTPS', 445: 'SMB', 465: 'SMTPS', 514: 'Syslog', 515: 'LPD', 548: 'AFP', 587: 'Submission', 623: 'IPMI',
  631: 'IPP', 636: 'LDAPS', 853: 'DNS over TLS', 873: 'rsync', 993: 'IMAPS', 995: 'POP3S', 1080: 'SOCKS',
  1194: 'OpenVPN', 1433: 'MS SQL', 1521: 'Oracle', 1723: 'PPTP', 1883: 'MQTT', 2049: 'NFS', 2375: 'Docker',
  2376: 'Docker TLS', 3000: 'HTTP (dev)', 3128: 'Squid', 3260: 'iSCSI', 3306: 'MySQL', 3389: 'RDP', 3478: 'STUN',
  5000: 'HTTP (alt)', 5060: 'SIP', 5432: 'PostgreSQL', 5601: 'Kibana', 5672: 'AMQP', 5900: 'VNC', 5901: 'VNC :1',
  5985: 'WinRM', 5986: 'WinRM TLS', 6379: 'Redis', 6443: 'Kubernetes API', 8006: 'Proxmox VE', 8080: 'HTTP (alt)',
  8081: 'HTTP (alt)', 8291: 'WinBox', 8443: 'HTTPS (alt)', 8728: 'MikroTik API', 8729: 'MikroTik API TLS',
  8883: 'MQTT TLS', 8888: 'HTTP (alt)', 9000: 'HTTP (alt)', 9090: 'Prometheus / Cockpit', 9100: 'Printer / node exporter',
  9200: 'Elasticsearch', 10000: 'Webmin', 11211: 'Memcached', 27017: 'MongoDB', 51820: 'WireGuard',
};

/** The common ports a quick scan checks: remote access, web, mail, files,
 *  databases and the management ports of servers and network gear. */
export const COMMON_PORTS: number[] = Object.keys(SERVICES).map(Number).sort((a, b) => a - b);

export type PortPreset = 'common' | 'well-known' | 'all' | 'custom';

/** A preset's ports; `custom` reads `spec`. */
export function presetPorts(preset: PortPreset, spec = ''): number[] {
  if (preset === 'common') return [...COMMON_PORTS];
  if (preset === 'well-known') return range(1, 1024);
  if (preset === 'all') return range(1, 65535);
  return parsePorts(spec).ports;
}

function range(from: number, to: number): number[] {
  const out: number[] = [];
  for (let p = from; p <= to; p++) out.push(p);
  return out;
}

/**
 * Ports as typed: `22, 80, 443, 8000-8100` (spaces, commas or semicolons
 * between). Sorted, each once; `error` names the first part that is not a
 * port or range.
 */
export function parsePorts(spec: string): { ports: number[]; error: string | null } {
  const set = new Set<number>();
  for (const raw of spec.split(/[\s,;]+/)) {
    const part = raw.trim();
    if (!part) continue;
    const m = /^(\d{1,5})(?:\s*-\s*(\d{1,5}))?$/.exec(part);
    if (!m) return { ports: [], error: part };
    const a = Number(m[1]);
    const b = m[2] ? Number(m[2]) : a;
    if (a < 1 || b < 1 || a > 65535 || b > 65535) return { ports: [], error: part };
    const [lo, hi] = a <= b ? [a, b] : [b, a];
    for (let p = lo; p <= hi; p++) set.add(p);
  }
  return { ports: [...set].sort((x, y) => x - y), error: null };
}

export function serviceName(port: number): string {
  return SERVICES[port] ?? '';
}

/** A MAC address as six bytes, from any usual spelling; null if it is not one. */
export function parseMac(text: string): number[] | null {
  const hex = text.replace(/[\s:.-]/g, '');
  if (!/^[0-9a-fA-F]{12}$/.test(hex)) return null;
  return Array.from({ length: 6 }, (_, i) => parseInt(hex.slice(i * 2, i * 2 + 2), 16));
}

/** `AA:BB:CC:DD:EE:FF`, or the text unchanged if it is not a MAC. */
export function formatMac(text: string): string {
  const b = parseMac(text);
  return b ? b.map((x) => x.toString(16).padStart(2, '0').toUpperCase()).join(':') : text;
}

export interface PingStats {
  sent: number;
  received: number;
  /** Per cent lost, 0-100. */
  loss: number;
  min: number | null;
  avg: number | null;
  max: number | null;
}

/** Statistics of a run, from each ping's round trip (null: no answer). */
export function pingStats(times: (number | null)[]): PingStats {
  const got = times.filter((t): t is number => t != null);
  const sent = times.length;
  return {
    sent,
    received: got.length,
    loss: sent ? Math.round(((sent - got.length) / sent) * 100) : 0,
    min: got.length ? Math.min(...got) : null,
    avg: got.length ? got.reduce((a, b) => a + b, 0) / got.length : null,
    max: got.length ? Math.max(...got) : null,
  };
}

/** A round trip as people read it: `0.4 ms`, `12 ms`. */
export function formatMs(ms: number | null): string {
  if (ms == null) return '–';
  return ms < 10 ? `${ms.toFixed(1)} ms` : `${Math.round(ms)} ms`;
}
