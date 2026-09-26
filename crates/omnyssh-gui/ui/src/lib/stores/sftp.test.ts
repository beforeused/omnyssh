import { describe, expect, it } from 'vitest';
import { get } from 'svelte/store';
import type { FileEntryDto } from '$lib/bindings';
import {
  sftp,
  newSession,
  applyListing,
  toggleMark,
  markedEntries,
  mergeRefresh,
  applyOpDone,
  applyEditSync,
  selectOnly,
  selectRange,
  selectAll,
  formatBytes,
  type Pane,
  type SftpSession
} from './sftp';

// The dual-pane browser's navigation/selection logic lives as pure reducers so it is
// unit-testable without a Tauri runtime (tech-gui.md §3.2, §6.4). Transfers have
// their own store (transfers.test.ts).

function entry(name: string, isDir = false, size = 0): FileEntryDto {
  return { name, path: `/srv/${name}`, size, isDir, isLink: false };
}

function paneWith(entries: FileEntryDto[], marked: string[] = []): Pane {
  return { path: '/srv', entries, loading: false, marked: new Set(marked) };
}

describe('sftp reducers', () => {
  it('starts a session connecting with both panes empty and loading', () => {
    const s = newSession('web-1');
    expect(s.status).toBe('connecting');
    expect(s.local.loading).toBe(true);
    expect(s.remote.loading).toBe(true);
    expect(s.local.entries).toEqual([]);
    expect(s.pending).toEqual([]);
  });

  it('applyListing replaces entries at a path and clears marks + loading', () => {
    const pane = paneWith([entry('a')], ['/srv/a']);
    const next = applyListing(pane, '/etc', [entry('b'), entry('c')]);
    expect(next.path).toBe('/etc');
    expect(next.entries.map((e) => e.name)).toEqual(['b', 'c']);
    expect(next.loading).toBe(false);
    // Navigating clears the previous directory's marks.
    expect(next.marked.size).toBe(0);
  });

  it('applyListing keeps the surviving selection when the same folder is re-listed', () => {
    const pane = paneWith([entry('a'), entry('b')], ['/srv/a', '/srv/b']);
    // `b` was deleted meanwhile; `c` landed from a transfer.
    const next = applyListing(pane, '/srv', [entry('a'), entry('c')]);
    expect([...next.marked]).toEqual(['/srv/a']);
  });

  it('selectOnly / selectRange / selectAll follow file-manager conventions', () => {
    let pane = paneWith([
      { name: '..', path: '/', size: 0, isDir: true, isLink: false },
      entry('a'),
      entry('b'),
      entry('c'),
      entry('d')
    ]);
    pane = selectOnly(pane, '/srv/b');
    expect([...pane.marked]).toEqual(['/srv/b']);
    // Shift-click extends from the anchor, in either direction.
    pane = selectRange(pane, '/srv/d');
    expect(markedEntries(pane).map((e) => e.name)).toEqual(['b', 'c', 'd']);
    pane = selectRange(pane, '/srv/a');
    expect(markedEntries(pane).map((e) => e.name)).toEqual(['a', 'b']);
    // Cmd-click toggles and moves the anchor.
    pane = toggleMark(pane, '/srv/d');
    expect(markedEntries(pane).map((e) => e.name)).toEqual(['a', 'b', 'd']);
    expect(pane.anchor).toBe('/srv/d');
    // Select-all never includes `..`.
    expect(markedEntries(selectAll(pane)).map((e) => e.name)).toEqual(['a', 'b', 'c', 'd']);
  });

  it('applyEditSync queues one question per file and clears it on a later status', () => {
    let s = newSession('web-1');
    s = applyEditSync(s, { path: '/etc/app.conf', state: 'modified' });
    s = applyEditSync(s, { path: '/etc/app.conf', state: 'modified' });
    expect(s.editPrompts).toEqual([{ path: '/etc/app.conf', kind: 'modified' }]);
    // The upload found the server copy changed: the question becomes a conflict.
    s = applyEditSync(s, { path: '/etc/app.conf', state: 'conflict' });
    expect(s.editPrompts).toEqual([{ path: '/etc/app.conf', kind: 'conflict' }]);
    s = applyEditSync(s, { path: '/etc/app.conf', state: 'synced' });
    expect(s.editPrompts).toEqual([]);
    expect(s.edit).toEqual({ path: '/etc/app.conf', state: 'synced' });
  });


  it('toggleMark marks and unmarks, and markedEntries keeps listing order', () => {
    let pane = paneWith([entry('a'), entry('b'), entry('c')]);
    pane = toggleMark(pane, '/srv/c');
    pane = toggleMark(pane, '/srv/a');
    expect(markedEntries(pane).map((e) => e.name)).toEqual(['a', 'c']);
    // Toggling an already-marked path removes it.
    pane = toggleMark(pane, '/srv/a');
    expect(markedEntries(pane).map((e) => e.name)).toEqual(['c']);
  });

  it('mergeRefresh widens two different sides to both', () => {
    expect(mergeRefresh(undefined, 'remote')).toBe('remote');
    expect(mergeRefresh('remote', 'remote')).toBe('remote');
    expect(mergeRefresh('local', 'remote')).toBe('both');
    expect(mergeRefresh('both', 'local')).toBe('both');
  });

  it('applyOpDone pops the front op (FIFO) and records its refresh', () => {
    const s: SftpSession = {
      ...newSession('web-1'),
      pending: [
        { kind: 'rename', name: 'a', refresh: 'remote' },
        { kind: 'mkdir', refresh: 'remote' }
      ]
    };
    const next = applyOpDone(s, true);
    expect(next.pending.map((p) => p.kind)).toEqual(['mkdir']);
    expect(next.refresh).toBe('remote');
    expect(next.error).toBeUndefined();
  });

  it('applyOpDone surfaces the error message on failure', () => {
    const s: SftpSession = {
      ...newSession('web-1'),
      pending: [{ kind: 'delete', name: 'x', refresh: 'remote' }]
    };
    const next = applyOpDone(s, false, 'permission denied');
    expect(next.error).toBe('permission denied');
    expect(next.pending).toEqual([]);
  });

  it('applyOpDone keeps a prior op error on a later success (no mid-batch masking)', () => {
    // A batch of [delete non-empty folder (fails), delete sibling (ok)] must not let the
    // sibling's success hide the folder's failure — the error persists.
    let s: SftpSession = {
      ...newSession('web-1'),
      pending: [
        { kind: 'delete', name: 'logs', refresh: 'remote' },
        { kind: 'delete', name: 'notes.txt', refresh: 'remote' }
      ]
    };
    s = applyOpDone(s, false, 'directory not empty');
    expect(s.error).toBe('directory not empty');
    s = applyOpDone(s, true);
    expect(s.error).toBe('directory not empty');
    expect(s.pending).toEqual([]);
  });

  it('formatBytes is human readable', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });
});

describe('sftp store', () => {
  it('keeps concurrent sessions isolated and prunes on close', () => {
    sftp.open(1, 'web-1');
    sftp.open(2, 'db-1');
    sftp.listing(1, 'remote', '/a', [entry('one')]);
    sftp.listing(2, 'remote', '/b', [entry('two'), entry('three')]);

    expect(get(sftp).get(1)?.remote.path).toBe('/a');
    expect(get(sftp).get(1)?.remote.entries).toHaveLength(1);
    // A listing for tab 1 never leaks into tab 2 — the store is keyed by session id.
    expect(get(sftp).get(2)?.remote.path).toBe('/b');
    expect(get(sftp).get(2)?.remote.entries).toHaveLength(2);

    sftp.remove(1);
    expect(get(sftp).has(1)).toBe(false);
    expect(get(sftp).has(2)).toBe(true);
    sftp.remove(2);
  });

  it('ignores mutations targeting an unknown (closed) session', () => {
    sftp.listing(999, 'remote', '/gone', [entry('x')]);
    expect(get(sftp).has(999)).toBe(false);
  });

  it('clearError drops a lingering batch error when a new batch is enqueued', () => {
    sftp.open(1, 'web-1');
    sftp.pushOp(1, { kind: 'delete', name: 'logs', refresh: 'remote' });
    sftp.opDone(1, false, 'directory not empty');
    expect(get(sftp).get(1)?.error).toBe('directory not empty');
    sftp.clearError(1);
    expect(get(sftp).get(1)?.error).toBeUndefined();
    sftp.remove(1);
  });

  it('sessionError clears the remote pane loading so a failed listing never sticks, but leaves a live local load', () => {
    sftp.open(1, 'web-1');
    sftp.beginLoading(1, 'remote');
    sftp.beginLoading(1, 'local');
    sftp.sessionError(1, 'ListDir failed: connection reset');
    const s = get(sftp).get(1);
    expect(s?.error).toBe('ListDir failed: connection reset');
    expect(s?.remote.loading).toBe(false);
    // A remote failure must not drop a legitimately in-flight local listing's spinner.
    expect(s?.local.loading).toBe(true);
    sftp.remove(1);
  });
});
