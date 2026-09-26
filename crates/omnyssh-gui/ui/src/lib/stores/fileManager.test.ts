import { describe, expect, it } from 'vitest';
import type { FileEntryDto } from '$lib/bindings';
import { viewEntries, toggleSort, breadcrumbs, permString, parentPath, isArchive } from './sftp';
import { installStateFrom } from './vpn';
import { isActive } from './transfers';

function e(name: string, isDir = false, size = 0, modified?: number): FileEntryDto {
  return { name, path: `/srv/${name}`, size, isDir, isLink: false, modified: modified ?? null, permissions: null };
}

const LIST = [
  e('..', true),
  e('b.txt', false, 300, 30),
  e('.env', false, 10, 50),
  e('logs', true, 0, 10),
  e('a10.log', false, 5, 20),
  e('a2.log', false, 900, 40)
];

describe('file-manager view', () => {
  it('keeps .. on top and folders first, sorting names naturally', () => {
    const v = viewEntries(LIST, { showHidden: true, filter: '', sort: { key: 'name', dir: 'asc' } });
    expect(v.map((x) => x.name)).toEqual(['..', 'logs', '.env', 'a2.log', 'a10.log', 'b.txt']);
  });

  it('hides dotfiles, filters by name, and sorts by size or date', () => {
    const noHidden = viewEntries(LIST, { showHidden: false, filter: '', sort: { key: 'name', dir: 'asc' } });
    expect(noHidden.some((x) => x.name === '.env')).toBe(false);
    const filtered = viewEntries(LIST, { showHidden: true, filter: 'LOG', sort: { key: 'name', dir: 'asc' } });
    expect(filtered.map((x) => x.name)).toEqual(['..', 'logs', 'a2.log', 'a10.log']);
    const bySize = viewEntries(LIST, { showHidden: true, filter: '', sort: { key: 'size', dir: 'desc' } });
    expect(bySize.slice(2).map((x) => x.name)).toEqual(['a2.log', 'b.txt', '.env', 'a10.log']);
    const newest = viewEntries(LIST, { showHidden: true, filter: '', sort: { key: 'modified', dir: 'desc' } });
    expect(newest.slice(2).map((x) => x.name)).toEqual(['.env', 'a2.log', 'b.txt', 'a10.log']);
  });

  it('toggles the sort direction on the same column, sensible default on a new one', () => {
    expect(toggleSort({ key: 'name', dir: 'asc' }, 'name')).toEqual({ key: 'name', dir: 'desc' });
    expect(toggleSort({ key: 'name', dir: 'asc' }, 'modified')).toEqual({ key: 'modified', dir: 'desc' });
  });

  it('builds breadcrumbs for POSIX and Windows paths', () => {
    expect(breadcrumbs('/var/www/site')).toEqual([
      { label: '/', path: '/' },
      { label: 'var', path: '/var' },
      { label: 'www', path: '/var/www' },
      { label: 'site', path: '/var/www/site' }
    ]);
    expect(breadcrumbs('C:\\Users\\me').map((c) => c.path)).toEqual(['C:\\', 'C:\\Users', 'C:\\Users\\me']);
    expect(parentPath('/var/www')).toBe('/var');
    expect(parentPath('/var')).toBe('/');
  });

  it('renders permission bits like ls -l', () => {
    expect(permString(0o755, true)).toBe('drwxr-xr-x');
    expect(permString(0o640, false)).toBe('-rw-r-----');
    expect(permString(0o4755, false)).toBe('-rwsr-xr-x');
    expect(permString(0o1777, true)).toBe('drwxrwxrwt');
    expect(permString(null, false)).toBe('');
  });

  it('recognises archives the server can unpack', () => {
    expect(['a.tar.gz', 'b.tgz', 'c.zip', 'd.sql.gz', 'e.tar.xz'].every(isArchive)).toBe(true);
    expect(isArchive('notes.txt')).toBe(false);
  });
});

describe('vpn install progress and resumed transfers', () => {
  it('turns install events into a percentage and states', () => {
    expect(installStateFrom({ stage: 'downloading', done: 50, total: 200, error: null })).toEqual({
      stage: 'downloading',
      percent: 25
    });
    expect(installStateFrom({ stage: 'failed', done: 0, total: 0, error: 'checksum' })).toEqual({
      stage: 'failed',
      error: 'checksum'
    });
    expect(installStateFrom({ stage: 'done', done: 0, total: 0, error: null })).toEqual({ stage: 'done' });
  });

  it('counts a reconnecting transfer as still active', () => {
    expect(isActive('reconnecting')).toBe(true);
    expect(isActive('done')).toBe(false);
  });
});
