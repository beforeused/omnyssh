import type { HostDto, HostInputDto, MonitorModeDto } from '$lib/bindings';

export const HOST_SHARE_PREFIX = 'omnyssh-share:v1:';
export const MAX_HOST_SHARE_LENGTH = 16 * 1024;

export interface SharedHost {
  version: 1;
  name: string;
  hostname: string;
  user: string;
  port: number;
  monitoring: MonitorModeDto;
  monitorPort?: number;
  vpn?: string;
}

const ALLOWED_KEYS = new Set([
  'version',
  'name',
  'hostname',
  'user',
  'port',
  'monitoring',
  'monitorPort',
  'vpn'
]);

function encodeBase64Url(value: string): string {
  const bytes = new TextEncoder().encode(value);
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

function decodeBase64Url(value: string): string {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) throw new Error('Invalid share code');
  const padding = '='.repeat((4 - (value.length % 4)) % 4);
  const binary = atob(value.replaceAll('-', '+').replaceAll('_', '/') + padding);
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
  return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
}

function requiredString(value: unknown, field: string, maxLength: number): string {
  if (typeof value !== 'string') throw new Error(`Invalid ${field}`);
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > maxLength || /[\0\r\n]/.test(trimmed)) {
    throw new Error(`Invalid ${field}`);
  }
  return trimmed;
}

function port(value: unknown, field: string): number {
  if (!Number.isInteger(value) || Number(value) < 1 || Number(value) > 65535) {
    throw new Error(`Invalid ${field}`);
  }
  return Number(value);
}

/** Encode only the minimum connection metadata. Host passwords, key paths,
 * passphrases, notes, tags, and VPN profile contents never enter the payload. */
export function encodeSharedHost(host: HostDto): string {
  const payload: SharedHost = {
    version: 1,
    name: requiredString(host.name, 'name', 128),
    hostname: requiredString(host.hostname, 'hostname', 255),
    user: requiredString(host.user, 'user', 128),
    port: port(host.port, 'port'),
    monitoring: host.monitoring
  };
  if (host.monitoring === 'tcpPort' && host.monitorPort != null) {
    payload.monitorPort = port(host.monitorPort, 'monitorPort');
  }
  if (host.vpn) payload.vpn = requiredString(host.vpn, 'vpn', 128);

  const result = HOST_SHARE_PREFIX + encodeBase64Url(JSON.stringify(payload));
  if (result.length > MAX_HOST_SHARE_LENGTH) throw new Error('Share code is too long');
  return result;
}

export function isHostShareText(text: string): boolean {
  return text.trim().startsWith('omnyssh-share:');
}

/** Strictly decode an untrusted clipboard payload. Unknown fields are rejected so
 * a future or malicious sender cannot smuggle commands or secrets through import. */
export function decodeSharedHost(text: string): SharedHost {
  const token = text.trim();
  if (!token.startsWith(HOST_SHARE_PREFIX) || token.length > MAX_HOST_SHARE_LENGTH) {
    throw new Error('Unsupported or invalid OmnySSH share code');
  }

  let raw: unknown;
  try {
    raw = JSON.parse(decodeBase64Url(token.slice(HOST_SHARE_PREFIX.length)));
  } catch {
    throw new Error('Invalid OmnySSH share code');
  }
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) {
    throw new Error('Invalid OmnySSH share code');
  }
  const object = raw as Record<string, unknown>;
  if (Object.keys(object).some((key) => !ALLOWED_KEYS.has(key)) || object.version !== 1) {
    throw new Error('Unsupported OmnySSH share code');
  }
  const monitoring = object.monitoring;
  if (monitoring !== 'ssh' && monitoring !== 'tcpPort') {
    throw new Error('Invalid monitoring mode');
  }

  const shared: SharedHost = {
    version: 1,
    name: requiredString(object.name, 'name', 128),
    hostname: requiredString(object.hostname, 'hostname', 255),
    user: requiredString(object.user, 'user', 128),
    port: port(object.port, 'port'),
    monitoring
  };
  if (object.monitorPort !== undefined) shared.monitorPort = port(object.monitorPort, 'monitorPort');
  if (object.vpn !== undefined) shared.vpn = requiredString(object.vpn, 'vpn', 128);
  return shared;
}

export function sharedHostInput(
  shared: SharedHost,
  name: string,
  identityFile?: string
): HostInputDto {
  return {
    name: requiredString(name, 'name', 128),
    hostname: shared.hostname,
    user: shared.user,
    port: shared.port,
    identityFile: identityFile?.trim() || undefined,
    tags: [],
    monitoring: shared.monitoring,
    monitorPort: shared.monitoring === 'tcpPort' ? shared.monitorPort : undefined,
    vpn: shared.vpn
  };
}

export function availableSharedHostName(name: string, existingNames: Iterable<string>): string {
  const existing = new Set(existingNames);
  if (!existing.has(name)) return name;
  let suffix = 2;
  while (existing.has(`${name} (${suffix})`)) suffix += 1;
  return `${name} (${suffix})`;
}
