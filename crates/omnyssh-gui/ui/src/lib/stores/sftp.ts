import { writable } from 'svelte/store';
import type { EditSyncStateDto, FileEntryDto } from '$lib/bindings';

// Per-session SFTP state for the dual-pane browser (tech-gui.md §3.2, §3.5), keyed by
// the backend public session id — the same id the `sftp-*` events carry, so the router
// routes each event to the right tab. Navigation/selection logic lives here as pure
// reducers so it is unit-testable; `SftpView.svelte` is a thin view + dispatcher.
// Uploads and downloads live in the transfers store: they run in the background on
// their own connections and never hold up these panes.

export type PaneSide = 'local' | 'remote';
type SftpStatus = 'connecting' | 'connected' | 'failed';
type OpKind = 'mkdir' | 'rename' | 'delete';

/** One side's browsing state: current directory, its entries, and the selection. */
export interface Pane {
  path: string;
  entries: FileEntryDto[];
  loading: boolean;
  /** Selected entry paths — the transfer/delete targets and what a drag carries. */
  marked: Set<string>;
  /** Where a Shift-click range starts. */
  anchor?: string;
  error?: string;
}

/** A question about an edited remote file. */
export interface EditPrompt {
  path: string;
  kind: 'modified' | 'conflict';
}

/** Sync state of a remote file open in an external editor. */
export interface EditStatus {
  path: string;
  state: EditSyncStateDto;
  error?: string;
}

/** A file preview (a remote `file-preview` event, or a local read) shown in a modal. */
interface Preview {
  path: string;
  content: string;
}

// A mutating op awaiting its `sftp-op-done`. The core processes commands sequentially,
// so op-done events arrive in issue order — this FIFO correlates each op-done to the op
// that produced it (the contract carries no op id, §4.3). `refresh` is the pane whose
// listing the op invalidates.
interface PendingOp {
  kind: OpKind;
  name?: string;
  refresh: PaneSide;
}

export interface SftpSession {
  hostName: string;
  status: SftpStatus;
  local: Pane;
  remote: Pane;
  pending: PendingOp[];
  preview?: Preview;
  /** The latest editor-sync status, shown in the status strip. */
  edit?: EditStatus;
  /** Edited files waiting on the user: a save to confirm, or a conflict to settle. */
  editPrompts: EditPrompt[];
  /** The last operation error, surfaced in the UI until the next successful action. */
  error?: string;
  /** Pane(s) to re-list once `pending` drains (a mutation changed the FS); the
   *  component performs the listing and clears this. */
  refresh?: PaneSide | 'both';
}

function emptyPane(): Pane {
  return { path: '', entries: [], loading: true, marked: new Set() };
}

/** A fresh session in the connecting state, both panes empty. */
export function newSession(hostName: string): SftpSession {
  return {
    hostName,
    status: 'connecting',
    local: emptyPane(),
    remote: emptyPane(),
    pending: [],
    editPrompts: []
  };
}

/** A directory listing landed for a pane: replace entries at `path`. Navigating
 *  elsewhere clears the selection; re-listing the same folder (a refresh while
 *  transfers land) keeps whatever is still there selected. */
export function applyListing(pane: Pane, path: string, entries: FileEntryDto[]): Pane {
  const same = path === pane.path;
  const present = new Set(entries.map((e) => e.path));
  const marked = same ? new Set([...pane.marked].filter((p) => present.has(p))) : new Set<string>();
  const anchor = same && pane.anchor && present.has(pane.anchor) ? pane.anchor : undefined;
  return { ...pane, path, entries, loading: false, marked, anchor, error: undefined };
}

/** Toggle an entry's selected state (Cmd/Ctrl-click, or its checkbox). */
export function toggleMark(pane: Pane, path: string): Pane {
  const marked = new Set(pane.marked);
  if (marked.has(path)) marked.delete(path);
  else marked.add(path);
  return { ...pane, marked, anchor: path };
}

/** Select just this entry (a plain click). */
export function selectOnly(pane: Pane, path: string): Pane {
  return { ...pane, marked: new Set([path]), anchor: path };
}

/** Select everything between the anchor and `path` (a Shift-click), in listing
 *  order. Without an anchor this is a plain selection. */
export function selectRange(pane: Pane, path: string, order: FileEntryDto[] = pane.entries): Pane {
  const selectable = order.filter((e) => e.name !== '..');
  const to = selectable.findIndex((e) => e.path === path);
  const from = pane.anchor ? selectable.findIndex((e) => e.path === pane.anchor) : -1;
  if (to < 0 || from < 0) return selectOnly(pane, path);
  const [a, b] = from < to ? [from, to] : [to, from];
  return {
    ...pane,
    marked: new Set(selectable.slice(a, b + 1).map((e) => e.path)),
    anchor: pane.anchor
  };
}

/** Select every entry but `..` (Cmd/Ctrl-A). */
export function selectAll(pane: Pane, order: FileEntryDto[] = pane.entries): Pane {
  return {
    ...pane,
    marked: new Set(order.filter((e) => e.name !== '..').map((e) => e.path))
  };
}

/** The marked entries in listing order — the stable sequence a batch transfer follows. */
export function markedEntries(pane: Pane): FileEntryDto[] {
  return pane.entries.filter((e) => pane.marked.has(e.path));
}

/** Human-readable byte size for a listing row or a transfer bar. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

/** Widen the pending refresh target: two different sides collapse to `both`. */
export function mergeRefresh(
  current: PaneSide | 'both' | undefined,
  next: PaneSide
): PaneSide | 'both' {
  if (!current || current === next) return next;
  return 'both';
}

/** Fold an `sftp-op-done` in: pop the front pending op (FIFO), record its refresh
 *  target, and surface any error. */
export function applyOpDone(session: SftpSession, ok: boolean, error?: string): SftpSession {
  if (session.pending.length === 0) return session;
  const [front, ...rest] = session.pending;
  return {
    ...session,
    pending: rest,
    refresh: mergeRefresh(session.refresh, front.refresh),
    // A later op's success must NOT wipe an earlier op's failure in the same batch — that
    // silently masks e.g. a non-empty-folder delete beside a deleted sibling. The error
    // persists until the next batch clears it (`clearError`, called on enqueue).
    error: ok ? session.error : (error ?? 'Operation failed')
  };
}

/** Fold an `edit-sync` event in: remember the latest status, and queue a question
 *  for the user (at most one per file) — "upload this save?" or a conflict. */
export function applyEditSync(session: SftpSession, status: EditStatus): SftpSession {
  const prompts = session.editPrompts.filter((p) => p.path !== status.path);
  if (status.state === 'modified' || status.state === 'conflict') {
    prompts.push({ path: status.path, kind: status.state });
  }
  return { ...session, edit: status, editPrompts: prompts };
}

function createSftp() {
  const { subscribe, update } = writable<Map<number, SftpSession>>(new Map());

  /** Apply `fn` to one session, no-op if the id is unknown (a closed tab). */
  function mut(id: number, fn: (s: SftpSession) => SftpSession): void {
    update((m) => {
      const session = m.get(id);
      if (!session) return m;
      const next = new Map(m);
      next.set(id, fn(session));
      return next;
    });
  }

  return {
    subscribe,
    /** Register a freshly opened session (called once `sftp_open` resolves). */
    open(id: number, hostName: string): void {
      update((m) => new Map(m).set(id, newSession(hostName)));
    },
    setStatus(id: number, status: SftpStatus): void {
      mut(id, (s) => ({ ...s, status }));
    },
    /** Mark a pane as loading before a listing request goes out. */
    beginLoading(id: number, side: PaneSide): void {
      mut(id, (s) => ({ ...s, [side]: { ...s[side], loading: true } }));
    },
    listing(id: number, side: PaneSide, path: string, entries: FileEntryDto[]): void {
      mut(id, (s) => ({ ...s, [side]: applyListing(s[side], path, entries) }));
    },
    paneError(id: number, side: PaneSide, error: string): void {
      mut(id, (s) => ({ ...s, [side]: { ...s[side], loading: false, error } }));
    },
    toggleMark(id: number, side: PaneSide, path: string): void {
      mut(id, (s) => ({ ...s, [side]: toggleMark(s[side], path) }));
    },
    selectOnly(id: number, side: PaneSide, path: string): void {
      mut(id, (s) => ({ ...s, [side]: selectOnly(s[side], path) }));
    },
    selectRange(id: number, side: PaneSide, path: string, order?: FileEntryDto[]): void {
      mut(id, (s) => ({ ...s, [side]: selectRange(s[side], path, order) }));
    },
    selectAll(id: number, side: PaneSide, order?: FileEntryDto[]): void {
      mut(id, (s) => ({ ...s, [side]: selectAll(s[side], order) }));
    },
    clearSelection(id: number, side: PaneSide): void {
      mut(id, (s) => ({ ...s, [side]: { ...s[side], marked: new Set<string>() } }));
    },
    pushOp(id: number, op: PendingOp): void {
      mut(id, (s) => ({ ...s, pending: [...s.pending, op] }));
    },
    opDone(id: number, ok: boolean, error?: string): void {
      mut(id, (s) => applyOpDone(s, ok, error));
    },
    setPreview(id: number, preview: Preview): void {
      mut(id, (s) => ({ ...s, preview }));
    },
    editSync(id: number, status: EditStatus): void {
      mut(id, (s) => applyEditSync(s, status));
    },
    /** The user answered a prompt about `path`: stop asking. */
    answerEditPrompt(id: number, path: string): void {
      mut(id, (s) => ({ ...s, editPrompts: s.editPrompts.filter((p) => p.path !== path) }));
    },
    clearPreview(id: number): void {
      mut(id, (s) => ({ ...s, preview: undefined }));
    },
    clearRefresh(id: number): void {
      mut(id, (s) => ({ ...s, refresh: undefined }));
    },
    /** Drop the surfaced op error — called when a new batch is enqueued, so a fresh
     *  action starts clean while a finished batch's error still lingered until now. */
    clearError(id: number): void {
      mut(id, (s) => ({ ...s, error: undefined }));
    },
    /** A soft error (e.g. a failed listing reported as `sftp-disconnected`, §4.3). The
     *  core emits it only for a remote `ListDir`, so clear just the remote pane's loading
     *  (leaving it set strands it on "Loading…"); a legit in-flight local listing keeps
     *  its own spinner. */
    sessionError(id: number, error: string): void {
      mut(id, (s) => ({ ...s, error, remote: { ...s.remote, loading: false } }));
    },
    remove(id: number): void {
      update((m) => {
        if (!m.has(id)) return m;
        const next = new Map(m);
        next.delete(id);
        return next;
      });
    }
  };
}

export const sftp = createSftp();

// ---------------------------------------------------------------------------
// File-manager view: what a pane shows, in which order
// ---------------------------------------------------------------------------

export type SortKey = 'name' | 'size' | 'modified';
export interface SortSpec {
  key: SortKey;
  dir: 'asc' | 'desc';
}

export interface ViewOptions {
  showHidden: boolean;
  filter: string;
  sort: SortSpec;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

/** The entries a pane shows: hidden files and the filter applied, folders first,
 *  then by the chosen column. `..` stays on top (and survives the filter). */
export function viewEntries(entries: FileEntryDto[], opts: ViewOptions): FileEntryDto[] {
  const q = opts.filter.trim().toLowerCase();
  const parent = entries.filter((e) => e.name === '..');
  const rest = entries.filter(
    (e) =>
      e.name !== '..' &&
      (opts.showHidden || !e.name.startsWith('.')) &&
      (!q || e.name.toLowerCase().includes(q))
  );
  const sign = opts.sort.dir === 'asc' ? 1 : -1;
  rest.sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    let c = 0;
    if (opts.sort.key === 'size') c = (a.isDir ? 0 : a.size) - (b.isDir ? 0 : b.size);
    else if (opts.sort.key === 'modified') c = (a.modified ?? 0) - (b.modified ?? 0);
    if (c === 0) c = collator.compare(a.name, b.name);
    return c * sign;
  });
  return [...parent, ...rest];
}

/** Clicking a column: sort by it, or flip the direction when it already is. */
export function toggleSort(sort: SortSpec, key: SortKey): SortSpec {
  if (sort.key === key) return { key, dir: sort.dir === 'asc' ? 'desc' : 'asc' };
  // Newest and biggest first are what people look for.
  return { key, dir: key === 'name' ? 'asc' : 'desc' };
}

/** Path segments for a breadcrumb bar; each carries the path it opens. */
export function breadcrumbs(path: string): Array<{ label: string; path: string }> {
  if (!path) return [];
  const windows = /^[A-Za-z]:[\\/]/.test(path);
  const sep = windows ? '\\' : '/';
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (windows) {
    const [drive, ...tail] = parts;
    const crumbs = [{ label: drive, path: `${drive}\\` }];
    let acc = `${drive}`;
    for (const p of tail) {
      acc += `${sep}${p}`;
      crumbs.push({ label: p, path: acc });
    }
    return crumbs;
  }
  const crumbs = [{ label: '/', path: '/' }];
  let acc = '';
  for (const p of parts) {
    acc += `/${p}`;
    crumbs.push({ label: p, path: acc });
  }
  return crumbs;
}

/** `drwxr-xr-x`-style text for permission bits (setuid/setgid/sticky included). */
export function permString(mode: number | null | undefined, isDir: boolean): string {
  if (mode == null) return '';
  const bit = (b: number, c: string) => (mode & b ? c : '-');
  const x = (b: number, special: number, set: string, unset: string) =>
    mode & special ? (mode & b ? set : unset) : bit(b, 'x');
  return (
    (isDir ? 'd' : '-') +
    bit(0o400, 'r') +
    bit(0o200, 'w') +
    x(0o100, 0o4000, 's', 'S') +
    bit(0o040, 'r') +
    bit(0o020, 'w') +
    x(0o010, 0o2000, 's', 'S') +
    bit(0o004, 'r') +
    bit(0o002, 'w') +
    x(0o001, 0o1000, 't', 'T')
  );
}

/** Folder containing `path` ('/' for a top-level entry). */
export function parentPath(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  if (cut < 0) return path;
  if (cut === 0) return path.slice(0, 1);
  if (/^[A-Za-z]:$/.test(path.slice(0, cut))) return path.slice(0, cut + 1);
  return path.slice(0, cut);
}

/** Archives the server can unpack (mirrors the core's `remote_fs::is_archive`). */
export function isArchive(name: string): boolean {
  return /\.(tar|tar\.gz|tgz|tar\.bz2|tbz2?|tar\.xz|txz|tar\.zst|zip|gz)$/i.test(name);
}
