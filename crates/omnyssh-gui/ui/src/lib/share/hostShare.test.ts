import { describe, expect, it } from 'vitest';
import type { HostDto } from '$lib/bindings';
import {
  availableSharedHostName,
  decodeSharedHost,
  encodeSharedHost,
  HOST_SHARE_PREFIX,
  sharedHostInput
} from './hostShare';

const host: HostDto = {
  name: 'Корпоративный сервер',
  hostname: '10.0.0.7',
  user: 'deploy',
  port: 2222,
  tags: ['secret-tag'],
  notes: 'never share this note',
  source: 'manual',
  hasKey: true,
  passwordAuthDisabled: true,
  monitoring: 'ssh',
  vpn: 'Corporate VPN'
};

describe('host share code', () => {
  it('round-trips Unicode connection metadata without secret-bearing fields', () => {
    const token = encodeSharedHost(host);
    expect(token.startsWith(HOST_SHARE_PREFIX)).toBe(true);
    expect(token).not.toContain('never share');
    expect(decodeSharedHost(token)).toEqual({
      version: 1,
      name: host.name,
      hostname: host.hostname,
      user: host.user,
      port: host.port,
      monitoring: 'ssh',
      vpn: host.vpn
    });
  });

  it('rejects unknown fields such as password or identityFile', () => {
    const encoded = btoa(
      JSON.stringify({
        version: 1,
        name: 'host',
        hostname: 'example.com',
        user: 'root',
        port: 22,
        monitoring: 'ssh',
        password: 'bad'
      })
    ).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
    expect(() => decodeSharedHost(HOST_SHARE_PREFIX + encoded)).toThrow('Unsupported');
  });

  it('builds a secret-free input and lets the recipient select a local key', () => {
    const input = sharedHostInput(decodeSharedHost(encodeSharedHost(host)), 'Imported', '/keys/me');
    expect(input).toMatchObject({ name: 'Imported', identityFile: '/keys/me', tags: [] });
    expect(input.password).toBeUndefined();
    expect(input.notes).toBeUndefined();
  });

  it('picks a non-destructive name when a host already exists', () => {
    expect(availableSharedHostName('Urent', ['Urent', 'Urent (2)'])).toBe('Urent (3)');
  });
});
