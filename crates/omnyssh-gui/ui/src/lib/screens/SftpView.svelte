<script lang="ts">
  // A live SFTP tab (tech-gui.md §3.2). One instance per SFTP session, kept mounted for
  // the session's life — hidden, not destroyed, when another entity is active — so pane
  // state survives tab switches. Opens the session on mount, drives both panes via the
  // sftp_* commands, and reads its per-session state from the sftp store (fed by the
  // `sftp-*` events, §3.4). Local browsing uses list_local_dir (returns directly);
  // remote uses sftp_list (arrives as an event).
  //
  // Uploads and downloads never block the panes: a batch is prepared (folders walked,
  // name conflicts collected), the user answers any conflicts, and the batch joins the
  // tab's background queue (TransferPanel). Files move in by toolbar, context menu,
  // dragging between the panes, or dropping them from the OS onto the remote pane.
  // Double-clicking a file opens it in the editor from Settings; each save of a remote
  // file asks before it is uploaded back. A terminal on the same host docks under the
  // panes (Ctrl+`), like an IDE's. Semantic tokens only (§5.1).
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { homeDir } from '@tauri-apps/api/path';
  import { Icon } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import ContextMenu, { type MenuItem } from '$lib/components/ContextMenu.svelte';
  import SftpPane, { type SelectMode, type PaneKey } from './SftpPane.svelte';
  import PermissionsDialog from './PermissionsDialog.svelte';
  import TransferPanel from './TransferPanel.svelte';
  import ConflictDialog from './ConflictDialog.svelte';
  import TerminalPane from './TerminalPane.svelte';
  import type {
    ArchiveFormatDto,
    LocalFsOpDto,
    RemoteFsOpDto,
    ConflictActionDto,
    ConflictResolutionDto,
    FileEntryDto,
    TransferConflictDto,
    TransferDirectionDto
  } from '$lib/bindings';
  import { sessions, type Session } from '$lib/stores/sessions';
  import { registerCloseGuard } from '$lib/stores/navigation';
  import {
    sftp,
    markedEntries,
    viewEntries,
    toggleSort,
    parentPath,
    isArchive,
    type PaneSide,
    type SortKey
  } from '$lib/stores/sftp';
  import { transfers, EMPTY_QUEUE, isActive } from '$lib/stores/transfers';
  import { editor, showHidden, fileSort, bookmarks, toggleBookmark } from '$lib/stores/settings';
  import { lastError } from '$lib/stores/notifications';
  import { onTerminalExit } from '$lib/ipc/router';
  import { t } from '$lib/i18n';
  import {
    sftpOpen,
    sftpList,
    sftpClose,
    sftpMkdir,
    sftpRename,
    sftpPreview,
    listLocalDir,
    previewLocalFile,
    transferPrepare,
    transferCommit,
    transferDiscard,
    transferCancel,
    transferRetry,
    transferForget,
    openLocalFile,
    editRemoteFile,
    editResolveConflict,
    editConfirmUpload,
    remoteFsOp,
    localFsOp
  } from '$lib/ipc/commands';

  let { session, active }: { session: Session; active: boolean } = $props();

  let backendId = $state<number | undefined>(undefined);
  let openError = $state<string | undefined>(undefined);
  let destroyed = false;
  let mirrored: string | undefined;
  let root = $state<HTMLDivElement | undefined>(undefined);

  // Queued mutations (mkdir/rename/delete), dispatched one at a time (see the pump
  // effect). The core's SFTP command channel is bounded and drops on overflow, so a
  // large batch fired at once would silently lose commands and wedge the op-done
  // FIFO; gating on the previous op's completion keeps at most one outstanding.
  let outbox = $state<Array<() => void>>([]);

  // A pending mkdir/rename input. Rename carries the entry being renamed.
  type PromptKind = 'mkdir' | 'rename' | 'newFile' | 'compress' | 'copyTo' | 'moveTo';
  let prompt = $state<{
    kind: PromptKind;
    side: PaneSide;
    value: string;
    targets: FileEntryDto[];
    format?: ArchiveFormatDto;
  } | null>(null);
  /** Remote entries waiting for "delete for good?". */
  let confirmDelete = $state<FileEntryDto[] | null>(null);
  /** An archive waiting for "extract here (overwrites)?". */
  let confirmExtract = $state<FileEntryDto | null>(null);
  /** Remote entries whose permissions are being edited. */
  let permsFor = $state<FileEntryDto[] | null>(null);
  /** Server-side file operations in flight. */
  let working = $state(0);
  /** A short note in the status strip ("Path copied"). */
  let note = $state<string | null>(null);
  let localFilter = $state('');
  let remoteFilter = $state('');

  /** Transfer batches being walked / checked for conflicts. */
  let preparing = $state(0);
  /** The file currently being fetched to open in the editor. */
  let opening = $state<string | undefined>(undefined);
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  // One conflict question at a time; batches queue up behind each other.
  let conflictAsk = $state<{
    conflict: TransferConflictDto;
    remaining: number;
    resolve: (answer: { action: ConflictActionDto; applyToAll: boolean } | null) => void;
  } | null>(null);
  let conflictChain: Promise<unknown> = Promise.resolve();

  // Drag state: an in-app drag of entries from one pane, or an OS file drag.
  let drag = $state<{ side: PaneSide; entries: FileEntryDto[]; x: number; y: number } | null>(null);
  let dropTarget = $state<{ side: PaneSide; dir: string | null } | null>(null);

  const view = $derived(backendId != null ? $sftp.get(backendId) : undefined);
  const queue = $derived((backendId != null ? $transfers.get(backendId) : undefined) ?? EMPTY_QUEUE);
  const showQueue = $derived(queue.items.length > 0 || preparing > 0);

  const localMarked = $derived(view ? markedEntries(view.local) : []);
  const remoteMarked = $derived(view ? markedEntries(view.remote) : []);
  const singleRemoteMark = $derived(remoteMarked.length === 1 ? remoteMarked[0] : undefined);

  // What each pane lists: hidden files and its filter applied, sorted by column.
  const localEntries = $derived(
    view
      ? viewEntries(view.local.entries, { showHidden: $showHidden, filter: localFilter, sort: $fileSort })
      : []
  );
  const remoteEntries = $derived(
    view
      ? viewEntries(view.remote.entries, { showHidden: $showHidden, filter: remoteFilter, sort: $fileSort })
      : []
  );
  const bookmarkKey = (side: PaneSide): string => (side === 'local' ? 'local' : `remote:${session.hostName}`);
  const localBookmarked = $derived(!!view && ($bookmarks.local ?? []).includes(view.local.path));
  const remoteBookmarked = $derived(
    !!view && ($bookmarks[`remote:${session.hostName}`] ?? []).includes(view.remote.path)
  );

  // A new folder starts unfiltered.
  let lastLocalPath = '';
  let lastRemotePath = '';
  $effect(() => {
    const lp = view?.local.path ?? '';
    const rp = view?.remote.path ?? '';
    if (lp !== lastLocalPath) {
      lastLocalPath = lp;
      localFilter = '';
    }
    if (rp !== lastRemotePath) {
      lastRemotePath = rp;
      remoteFilter = '';
    }
  });
  const editPrompt = $derived(view?.editPrompts[0]);

  function errMsg(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  function joinRemote(dir: string, name: string): string {
    return dir.endsWith('/') ? `${dir}${name}` : `${dir}/${name}`;
  }

  function joinLocal(dir: string, name: string): string {
    const sep = dir.includes('\\') && !dir.includes('/') ? '\\' : '/';
    return dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`;
  }

  function baseName(path: string): string {
    return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
  }

  async function refreshLocal(path: string): Promise<void> {
    const id = backendId;
    if (id == null) return;
    sftp.beginLoading(id, 'local');
    try {
      const entries = await listLocalDir(path);
      sftp.listing(id, 'local', path, entries);
    } catch (err) {
      sftp.paneError(id, 'local', errMsg(err));
    }
  }

  function refreshRemote(path: string): void {
    const id = backendId;
    if (id == null) return;
    sftp.beginLoading(id, 'remote');
    void sftpList(id, path).catch((err) => sftp.paneError(id, 'remote', errMsg(err)));
  }

  function refreshSide(side: PaneSide): void {
    if (!view) return;
    if (side === 'local') void refreshLocal(view.local.path);
    else refreshRemote(view.remote.path);
  }

  // Coalesce refreshes while files keep landing: at most one listing per pane per
  // REFRESH_DEBOUNCE_MS.
  const REFRESH_DEBOUNCE_MS = 700;
  const refreshTimers: Partial<Record<PaneSide, ReturnType<typeof setTimeout>>> = {};
  function scheduleRefresh(side: PaneSide): void {
    if (refreshTimers[side]) return;
    refreshTimers[side] = setTimeout(() => {
      refreshTimers[side] = undefined;
      refreshSide(side);
    }, REFRESH_DEBOUNCE_MS);
  }

  function samePath(a: string, b: string): boolean {
    const norm = (p: string) => (p.length > 1 ? p.replace(/[\\/]+$/, '') : p);
    return norm(a) === norm(b);
  }

  let unlistenDrop: (() => void) | undefined;
  let unregisterGuard: (() => void) | undefined;

  onMount(() => {
    void (async () => {
      let home = '/';
      try {
        home = await homeDir();
      } catch {
        home = '/';
      }
      let id: number;
      try {
        id = await sftpOpen(session.hostName);
      } catch (err) {
        sessions.setStatus(session.id, 'failed');
        openError = errMsg(err);
        lastError.set(errMsg(err));
        return;
      }
      if (destroyed) {
        void sftpClose(id).catch(() => {});
        return;
      }
      backendId = id;
      sftp.open(id, session.hostName);
      void refreshLocal(home);
      refreshRemote('/');
    })();

    // Files dropped from the OS (Finder, Explorer, a file manager) onto the remote
    // pane upload there. Absent outside Tauri (tests, vite preview).
    void (async () => {
      try {
        const { getCurrentWebview } = await import('@tauri-apps/api/webview');
        const off = await getCurrentWebview().onDragDropEvent((event) => {
          if (!active || !view) return;
          const p = event.payload;
          if (p.type === 'leave') {
            dropTarget = null;
            return;
          }
          const target = osDropTarget(p.position.x, p.position.y);
          if (p.type === 'drop') {
            dropTarget = null;
            if (target && p.paths.length) {
              void startTransfer('upload', p.paths, target.dir ?? view.remote.path);
            }
          } else {
            dropTarget = target;
          }
        });
        if (destroyed) off();
        else unlistenDrop = off;
      } catch {
        // Not under Tauri.
      }
    })();

    unregisterGuard = registerCloseGuard(session.id, confirmClose);
  });

  onDestroy(() => {
    destroyed = true;
    unlistenDrop?.();
    unregisterGuard?.();
    unlistenDockExit?.();
    for (const timer of Object.values(refreshTimers)) clearTimeout(timer);
    if (backendId != null) {
      void sftpClose(backendId).catch(() => {});
      sftp.remove(backendId);
      transfers.remove(backendId);
    }
  });

  /** Closing the tab cancels its transfers — ask first when some are still running. */
  async function confirmClose(): Promise<boolean> {
    const running = queue.items.filter((i) => isActive(i.state)).length;
    if (running === 0) return true;
    const message = $t('sftp.close.message', { count: running, host: session.hostName });
    try {
      const { ask } = await import('@tauri-apps/plugin-dialog');
      return await ask(message, {
        title: $t('sftp.close.title'),
        kind: 'warning',
        okLabel: $t('sftp.close.ok'),
        cancelLabel: $t('sftp.close.keep')
      });
    } catch {
      return window.confirm(message);
    }
  }

  // Mirror the store connection status to the sidebar dot (the sessions store is the
  // sidebar's source of truth); only on change, to avoid churning the sessions list.
  $effect(() => {
    if (view && view.status !== mirrored) {
      mirrored = view.status;
      sessions.setStatus(session.id, view.status);
    }
  });

  // Dispatch the next queued mutation once the previous one is acked (pending empty),
  // so at most one command is outstanding and the bounded core channel never overflows.
  $effect(() => {
    if (!view || view.pending.length > 0 || outbox.length === 0) return;
    const [next, ...rest] = outbox;
    outbox = rest;
    next();
  });

  // Re-list the affected pane once every queued mutation has drained — the FS changed
  // (§3.2). Gated on an empty outbox so a batch re-lists once at the end, not per op.
  $effect(() => {
    const id = backendId;
    if (id == null || !view || view.pending.length > 0 || outbox.length > 0 || !view.refresh) return;
    const target = view.refresh;
    sftp.clearRefresh(id);
    if (target === 'local' || target === 'both') void refreshLocal(view.local.path);
    if (target === 'remote' || target === 'both') refreshRemote(view.remote.path);
  });

  // Finished transfers landed files somewhere: re-list a pane showing that folder.
  $effect(() => {
    const id = backendId;
    if (id == null || !view) return;
    if (!queue.landed.local.length && !queue.landed.remote.length) return;
    const landed = transfers.takeLanded(id);
    if (landed.local.some((d) => samePath(d, view.local.path))) scheduleRefresh('local');
    if (landed.remote.some((d) => samePath(d, view.remote.path))) scheduleRefresh('remote');
  });

  // -- transfers ------------------------------------------------------------

  /** Prepare → (ask about conflicts) → enqueue. Resolves once the batch is queued. */
  async function startTransfer(
    direction: TransferDirectionDto,
    sources: string[],
    destDir: string
  ): Promise<void> {
    const id = backendId;
    if (id == null || sources.length === 0) return;
    preparing += 1;
    try {
      const batch = await transferPrepare(id, direction, sources, destDir);
      let resolutions: ConflictResolutionDto[] = [];
      if (batch.conflicts.length) {
        const answers = await resolveConflicts(batch.conflicts);
        if (answers === null) {
          void transferDiscard(id, batch.batchId).catch(() => {});
          return;
        }
        resolutions = answers;
      }
      const items = await transferCommit(id, batch.batchId, resolutions);
      transfers.add(id, items);
      // New folders were created at commit; show them in a pane looking at the target.
      const side: PaneSide = direction === 'upload' ? 'remote' : 'local';
      if (view && samePath(view[side].path, destDir)) scheduleRefresh(side);
    } catch (err) {
      lastError.set(errMsg(err));
    } finally {
      preparing -= 1;
    }
  }

  /** Ask about each conflict in turn ("apply to all" answers the rest). `null` when
   *  the user cancelled the batch. Batches wait their turn for the dialog. */
  function resolveConflicts(conflicts: TransferConflictDto[]): Promise<ConflictResolutionDto[] | null> {
    const fit = (action: ConflictActionDto, c: TransferConflictDto): ConflictActionDto =>
      action === 'replace' && c.existingIsDir ? 'skip' : action;
    const run = conflictChain.then(async () => {
      const out: ConflictResolutionDto[] = [];
      for (let i = 0; i < conflicts.length; i++) {
        const answer = await new Promise<{ action: ConflictActionDto; applyToAll: boolean } | null>(
          (resolve) => (conflictAsk = { conflict: conflicts[i], remaining: conflicts.length - i, resolve })
        );
        conflictAsk = null;
        if (!answer) return null;
        if (answer.applyToAll) {
          for (const c of conflicts.slice(i)) out.push({ index: c.index, action: fit(answer.action, c) });
          return out;
        }
        out.push({ index: conflicts[i].index, action: fit(answer.action, conflicts[i]) });
      }
      return out;
    });
    conflictChain = run.catch(() => {});
    return run;
  }

  function upload(entries: FileEntryDto[] = localMarked, destDir?: string): void {
    if (!view || entries.length === 0) return;
    void startTransfer(
      'upload',
      entries.map((e) => e.path),
      destDir ?? view.remote.path
    );
  }

  function download(entries: FileEntryDto[] = remoteMarked, destDir?: string): void {
    if (!view || entries.length === 0) return;
    void startTransfer(
      'download',
      entries.map((e) => e.path),
      destDir ?? view.local.path
    );
  }

  function cancelTransfers(ids: number[]): void {
    if (backendId != null) void transferCancel(backendId, ids).catch((e) => lastError.set(errMsg(e)));
  }

  function retryTransfers(ids: number[]): void {
    if (backendId != null) void transferRetry(backendId, ids).catch((e) => lastError.set(errMsg(e)));
  }

  function dismissTransfer(transferId: number): void {
    const id = backendId;
    if (id == null) return;
    transfers.dismiss(id, transferId);
    void transferForget(id, [transferId]).catch(() => {});
  }

  function clearTransfers(): void {
    const id = backendId;
    if (id == null) return;
    const removed = transfers.clearFinished(id, true);
    if (removed.length) void transferForget(id, removed).catch(() => {});
  }

  // -- opening files -----------------------------------------------------------

  async function openEntry(side: PaneSide, entry: FileEntryDto): Promise<void> {
    const id = backendId;
    if (id == null) return;
    const choice = get(editor);
    if (side === 'local') {
      try {
        await openLocalFile(entry.path, choice);
      } catch (err) {
        lastError.set(errMsg(err));
      }
      return;
    }
    opening = entry.name;
    try {
      await editRemoteFile(id, entry.path, choice);
    } catch (err) {
      lastError.set(errMsg(err));
    } finally {
      opening = undefined;
    }
  }

  async function preview(side: PaneSide, entry: FileEntryDto): Promise<void> {
    const id = backendId;
    if (id == null) return;
    if (side === 'local') {
      try {
        const content = await previewLocalFile(entry.path);
        sftp.setPreview(id, { path: entry.path, content });
      } catch (err) {
        lastError.set(errMsg(err));
      }
    } else {
      void sftpPreview(id, entry.path).catch((err) => lastError.set(errMsg(err)));
    }
  }

  async function answerEditConflict(path: string, overwrite: boolean): Promise<void> {
    const id = backendId;
    if (id == null) return;
    sftp.answerEditPrompt(id, path);
    try {
      await editResolveConflict(id, path, overwrite);
    } catch (err) {
      lastError.set(errMsg(err));
    }
  }

  async function answerEditUpload(path: string, upload: boolean): Promise<void> {
    const id = backendId;
    if (id == null) return;
    sftp.answerEditPrompt(id, path);
    try {
      await editConfirmUpload(id, path, upload);
    } catch (err) {
      lastError.set(errMsg(err));
    }
  }

  // -- docked terminal -----------------------------------------------------------

  const DOCK_HEIGHT_KEY = 'omnyssh-sftp-terminal-height';
  const DOCK_MIN = 120;
  /** Mounted: the PTY lives on while the dock is merely hidden. */
  let dockMounted = $state(false);
  let dockShown = $state(false);
  let dockEnded = $state(false);
  /** Bumped to start a fresh shell after the previous one ended. */
  let dockGeneration = $state(0);
  let dockHeight = $state(readDockHeight());
  let dockPane = $state<TerminalPane | undefined>(undefined);
  let dockStartDir = $state('');
  let unlistenDockExit: (() => void) | undefined;

  function readDockHeight(): number {
    try {
      const n = Number(localStorage.getItem(DOCK_HEIGHT_KEY));
      return Number.isFinite(n) && n >= DOCK_MIN ? n : 280;
    } catch {
      return 280;
    }
  }

  /** A shell-quoted `cd` into `dir` (single quotes survive any name). */
  function cdCommand(dir: string): string {
    return `cd '${dir.replace(/'/g, `'\\''`)}'\n`;
  }

  function toggleDock(): void {
    if (dockShown) {
      dockShown = false;
      return;
    }
    if (!dockMounted || dockEnded) openDock();
    else dockShown = true;
  }

  function openDock(dir = view?.remote.path): void {
    unlistenDockExit?.();
    dockStartDir = dir && dir !== '/' ? dir : '';
    dockEnded = false;
    dockGeneration += 1;
    dockMounted = true;
    dockShown = true;
  }

  function closeDock(): void {
    unlistenDockExit?.();
    dockMounted = false;
    dockShown = false;
    dockEnded = false;
  }

  function dockOpened(termId: number): void {
    unlistenDockExit?.();
    unlistenDockExit = onTerminalExit(termId, () => (dockEnded = true));
  }

  /** Right-click → "Open terminal here": reuse the live shell when there is one. */
  function terminalHere(dir: string): void {
    if (dockMounted && !dockEnded && dockPane) {
      dockShown = true;
      dockPane.type(cdCommand(dir));
    } else {
      openDock(dir);
    }
  }

  function resizeDock(e: PointerEvent): void {
    e.preventDefault();
    const startY = e.clientY;
    const startH = dockHeight;
    const max = Math.max(DOCK_MIN, (root?.clientHeight ?? 600) - 160);
    const move = (ev: PointerEvent) => {
      dockHeight = Math.min(max, Math.max(DOCK_MIN, startH + (startY - ev.clientY)));
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      try {
        localStorage.setItem(DOCK_HEIGHT_KEY, String(Math.round(dockHeight)));
      } catch {
        // Not persisted: fine.
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  function onWindowKey(e: KeyboardEvent): void {
    if (!active || !view) return;
    if (e.ctrlKey && !e.metaKey && !e.altKey && (e.key === '`' || e.code === 'Backquote')) {
      e.preventDefault();
      toggleDock();
    }
  }

  // The latest editor-sync note fades once it is good news.
  let editNoteHidden = $state(false);
  $effect(() => {
    const edit = view?.edit;
    editNoteHidden = false;
    if (edit?.state !== 'synced') return;
    const timer = setTimeout(() => (editNoteHidden = true), 4000);
    return () => clearTimeout(timer);
  });

  // -- browsing & mutations ------------------------------------------------------

  function navigate(side: PaneSide, entry: FileEntryDto): void {
    if (side === 'local') void refreshLocal(entry.path);
    else refreshRemote(entry.path);
  }

  function select(side: PaneSide, path: string, mode: SelectMode): void {
    const id = backendId;
    if (id == null) return;
    if (mode === 'toggle') sftp.toggleMark(id, side, path);
    else if (mode === 'range') sftp.selectRange(id, side, path, side === 'local' ? localEntries : remoteEntries);
    else sftp.selectOnly(id, side, path);
  }

  // If a mutating invoke itself rejects (it never does for a normal enqueue, but an IPC
  // failure could), pop its pending op so the dispatch pump does not wedge.
  function onDispatchError(id: number): (err: unknown) => void {
    return (err) => {
      lastError.set(errMsg(err));
      sftp.opDone(id, false, errMsg(err));
    };
  }

  function enqueue(...actions: Array<() => void>): void {
    if (!actions.length) return;
    // Clear the prior batch's lingering error only when starting from idle. Piling onto a
    // batch that is still draining must not wipe a failure it already recorded (that error
    // stays visible until the next fresh action — see applyOpDone).
    const draining = outbox.length > 0 || (view?.pending.length ?? 0) > 0;
    if (backendId != null && !draining) sftp.clearError(backendId);
    outbox = [...outbox, ...actions];
  }

  // -- file operations -----------------------------------------------------------

  function openPath(side: PaneSide, path: string): void {
    if (side === 'local') void refreshLocal(path);
    else refreshRemote(path);
  }

  function goUp(side: PaneSide): void {
    if (!view) return;
    const current = view[side].path;
    const up = parentPath(current);
    if (up && up !== current) openPath(side, up);
  }

  async function runRemote(op: RemoteFsOpDto): Promise<void> {
    const id = backendId;
    if (id == null) return;
    working += 1;
    try {
      await remoteFsOp(id, op);
    } catch (err) {
      lastError.set(errMsg(err));
    } finally {
      working -= 1;
      if (view) refreshRemote(view.remote.path);
    }
  }

  async function runLocal(op: LocalFsOpDto): Promise<void> {
    try {
      await localFsOp(op);
    } catch (err) {
      lastError.set(errMsg(err));
    } finally {
      if (view) void refreshLocal(view.local.path);
    }
  }

  /** Remote: ask, then delete recursively. Local: straight to the Trash. */
  function askDelete(side: PaneSide, entries: FileEntryDto[]): void {
    if (!entries.length) return;
    if (side === 'remote') confirmDelete = entries;
    else void runLocal({ kind: 'trash', paths: entries.map((e) => e.path) });
  }

  function openPrompt(kind: PromptKind, side: PaneSide = 'remote', targets: FileEntryDto[] = []): void {
    if (!view) return;
    const dir = view[side].path;
    if (kind === 'rename') {
      const target = targets[0] ?? (side === 'remote' ? singleRemoteMark : localMarked[0]);
      if (!target) return;
      prompt = { kind, side, value: target.name, targets: [target] };
    } else if (kind === 'compress') {
      const base = targets.length === 1 ? targets[0].name : 'archive';
      prompt = { kind, side, value: `${base}.tar.gz`, targets, format: 'tarGz' };
    } else if (kind === 'copyTo' || kind === 'moveTo') {
      prompt = { kind, side, value: dir, targets };
    } else {
      prompt = { kind, side, value: '', targets };
    }
  }

  /** Switching the archive format swaps the name's extension along with it. */
  function setFormat(format: ArchiveFormatDto): void {
    if (!prompt) return;
    const stem = prompt.value.replace(/(\.tar\.gz|\.zip)$/i, '');
    prompt = { ...prompt, format, value: `${stem}${format === 'zip' ? '.zip' : '.tar.gz'}` };
  }

  function submitPrompt(): void {
    const id = backendId;
    if (id == null || !view || !prompt) return;
    const value = prompt.value.trim();
    if (!value) return;
    const { kind, side, targets, format } = prompt;
    const dir = view[side].path;
    prompt = null;
    if (side === 'local') {
      if (kind === 'mkdir') void runLocal({ kind: 'mkdir', path: joinLocal(dir, value) });
      else if (kind === 'newFile') void runLocal({ kind: 'newFile', path: joinLocal(dir, value) });
      else if (kind === 'rename' && targets[0]) {
        void runLocal({ kind: 'rename', from: targets[0].path, to: joinLocal(dir, value) });
      }
      return;
    }
    const paths = targets.map((t) => t.path);
    switch (kind) {
      case 'mkdir':
        enqueue(() => {
          sftp.pushOp(id, { kind: 'mkdir', refresh: 'remote' });
          void sftpMkdir(id, joinRemote(dir, value)).catch(onDispatchError(id));
        });
        break;
      case 'rename': {
        const from = targets[0]?.path;
        if (!from) break;
        enqueue(() => {
          sftp.pushOp(id, { kind: 'rename', refresh: 'remote' });
          void sftpRename(id, from, joinRemote(dir, value)).catch(onDispatchError(id));
        });
        break;
      }
      case 'newFile':
        void runRemote({ kind: 'newFile', path: joinRemote(dir, value) });
        break;
      case 'compress':
        void runRemote({
          kind: 'compress',
          dir,
          names: targets.map((t) => t.name),
          archive: value,
          format: format ?? (value.toLowerCase().endsWith('.zip') ? 'zip' : 'tarGz')
        });
        break;
      case 'copyTo':
        void runRemote({ kind: 'copy', paths, dest: value });
        break;
      case 'moveTo':
        void runRemote({ kind: 'move', paths, dest: value });
        break;
    }
  }

  async function copyPaths(entries: FileEntryDto[]): Promise<void> {
    try {
      await navigator.clipboard.writeText(entries.map((e) => e.path).join('\n'));
      note = $t('fm.copied');
      setTimeout(() => (note = null), 2000);
    } catch (err) {
      lastError.set(errMsg(err));
    }
  }

  function paneKey(side: PaneSide, key: PaneKey): void {
    if (key === 'refresh') refreshSide(side);
    else if (key === 'delete') askDelete(side, side === 'local' ? localMarked : remoteMarked);
    else if (key === 'rename') openPrompt('rename', side);
  }

  function showBookmarks(side: PaneSide, x: number, y: number): void {
    const list = $bookmarks[bookmarkKey(side)] ?? [];
    menu = {
      x,
      y,
      items: list.length
        ? list.map((path) => ({ label: path, icon: 'folder' as const, run: () => openPath(side, path) }))
        : [{ label: $t('fm.noBookmarks'), disabled: true, run: () => {} }]
    };
  }

  function closePreview(): void {
    if (backendId != null) sftp.clearPreview(backendId);
  }

  // -- context menu ------------------------------------------------------------

  function showMenu(side: PaneSide, entry: FileEntryDto | null, x: number, y: number): void {
    if (!view) return;
    const selected = side === 'local' ? localMarked : remoteMarked;
    const many = entry ? selected.length > 1 && selected.some((e) => e.path === entry.path) : false;
    const targets = entry ? (many ? selected : [entry]) : [];
    const n = targets.length;
    const single = entry && !many ? entry : null;
    const items: MenuItem[] = [];
    if (single) {
      if (single.isDir) {
        items.push({ label: $t('sftp.menu.open'), icon: 'folder', run: () => navigate(side, single) });
      } else {
        items.push({
          label: side === 'remote' ? $t('sftp.menu.openInEditor') : $t('sftp.menu.open'),
          icon: 'external',
          run: () => void openEntry(side, single)
        });
        items.push({ label: $t('sftp.menu.quickLook'), icon: 'eye', run: () => void preview(side, single) });
        if (side === 'remote' && isArchive(single.name)) {
          items.push({ label: $t('fm.extract'), icon: 'archive', run: () => (confirmExtract = single) });
        }
      }
    }
    if (n) {
      if (side === 'local') {
        items.push({
          label: n > 1 ? $t('sftp.menu.uploadN', { count: n }) : $t('sftp.menu.upload'),
          icon: 'upload',
          run: () => upload(targets)
        });
      } else {
        items.push({
          label: n > 1 ? $t('sftp.menu.downloadN', { count: n }) : $t('sftp.menu.download'),
          icon: 'download',
          run: () => download(targets)
        });
      }
    }
    if (single) {
      items.push({ label: $t('sftp.menu.rename'), icon: 'edit', run: () => openPrompt('rename', side, [single]) });
    }
    if (side === 'remote' && n) {
      items.push({ label: $t('fm.copyTo'), icon: 'copy', run: () => openPrompt('copyTo', side, targets) });
      items.push({ label: $t('fm.moveTo'), icon: 'arrow-up', run: () => openPrompt('moveTo', side, targets) });
      items.push({ label: $t('fm.compress'), icon: 'archive', run: () => openPrompt('compress', side, targets) });
      items.push({ label: $t('fm.permissions'), icon: 'lock', run: () => (permsFor = targets) });
    }
    if (n) {
      items.push({ label: $t('fm.copyPath'), icon: 'copy', run: () => void copyPaths(targets) });
    }
    if (side === 'local' && single) {
      const dir = single.isDir ? single.path : parentPath(single.path);
      items.push({
        label: $t('fm.revealLocal'),
        icon: 'external',
        run: () => void openLocalFile(dir, { kind: 'system' }).catch((e) => lastError.set(errMsg(e)))
      });
    }
    if (n) {
      if (side === 'remote') {
        items.push({
          label: n > 1 ? $t('sftp.menu.deleteN', { count: n }) : $t('sftp.menu.delete'),
          icon: 'trash',
          danger: true,
          run: () => askDelete(side, targets)
        });
      } else {
        items.push({
          label: n > 1 ? $t('fm.trashN', { count: n }) : $t('fm.trash'),
          icon: 'trash',
          danger: true,
          run: () => askDelete(side, targets)
        });
      }
    }
    if (!entry) {
      items.push({ label: $t('sftp.menu.newFolder'), icon: 'plus', run: () => openPrompt('mkdir', side) });
      items.push({ label: $t('fm.newFile'), icon: 'file', run: () => openPrompt('newFile', side) });
    }
    if (side === 'remote') {
      const dir = single?.isDir ? single.path : view.remote.path;
      items.push({ label: $t('sftp.menu.terminalHere'), icon: 'terminal', run: () => terminalHere(dir) });
    }
    items.push({ label: $t('common.refresh'), icon: 'refresh', run: () => refreshSide(side) });
    menu = { x, y, items };
  }

  // -- files dragged in from the OS ---------------------------------------------

  // The drop position arrives labelled "physical", but only Windows reports device
  // pixels: macOS (WKWebView) and Linux (WebKitGTK) report CSS points already. Scaling
  // those again put a Retina drop at half its coordinates — over the local pane.
  const OS_DROP_IN_DEVICE_PIXELS = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent);

  /** Where files dragged in from the OS would land: a folder row in the remote pane,
   *  or the remote pane's current folder. Anywhere in the tab but the local pane
   *  counts as "the server" — the drop is an upload wherever it lands. */
  function osDropTarget(px: number, py: number): { side: PaneSide; dir: string | null } | null {
    const scale = OS_DROP_IN_DEVICE_PIXELS ? window.devicePixelRatio || 1 : 1;
    const x = px / scale;
    const y = py / scale;
    const exact = hitTest(x, y, 'remote');
    if (exact) return exact;
    const el = document.elementFromPoint(x, y);
    const overLocal = el?.closest<HTMLElement>('[data-drop-side="local"]');
    if (overLocal && root?.contains(overLocal)) return null;
    return { side: 'remote', dir: null };
  }

  // -- in-app drag between the panes -------------------------------------------

  /** The pane (limited to `side`) and folder row under a viewport point. */
  function hitTest(x: number, y: number, side: PaneSide): { side: PaneSide; dir: string | null } | null {
    const el = document.elementFromPoint(x, y);
    const paneEl = el?.closest<HTMLElement>('[data-drop-side]');
    if (!paneEl || !root?.contains(paneEl) || paneEl.dataset.dropSide !== side) return null;
    const dirEl = el?.closest<HTMLElement>('[data-drop-dir]');
    return { side, dir: dirEl?.dataset.dropDir ?? null };
  }

  function rowPointerDown(side: PaneSide, entry: FileEntryDto, e: PointerEvent): void {
    if (e.button !== 0 || e.shiftKey || e.metaKey || e.ctrlKey || !view) return;
    const id = backendId;
    if (id == null) return;
    const other: PaneSide = side === 'local' ? 'remote' : 'local';
    const start = { x: e.clientX, y: e.clientY };

    const move = (ev: PointerEvent) => {
      if (!drag) {
        if (Math.hypot(ev.clientX - start.x, ev.clientY - start.y) < 5 || !view) return;
        const pane = view[side];
        const carried = pane.marked.has(entry.path) ? markedEntries(pane) : [entry];
        if (!pane.marked.has(entry.path)) sftp.selectOnly(id, side, entry.path);
        drag = { side, entries: carried, x: ev.clientX, y: ev.clientY };
      } else {
        drag = { ...drag, x: ev.clientX, y: ev.clientY };
      }
      dropTarget = hitTest(ev.clientX, ev.clientY, other);
    };
    const end = (ev: PointerEvent | KeyboardEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', end);
      window.removeEventListener('pointercancel', end);
      window.removeEventListener('keydown', onKey);
      const carried = drag;
      const target = dropTarget;
      drag = null;
      dropTarget = null;
      if (!carried) return;
      // The release lands a click on whatever row is under it: swallow that one.
      const swallow = (c: Event) => {
        c.stopPropagation();
        c.preventDefault();
      };
      window.addEventListener('click', swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener('click', swallow, { capture: true }), 0);
      if (ev.type !== 'pointerup' || !target) return;
      if (carried.side === 'local') upload(carried.entries, target.dir ?? undefined);
      else download(carried.entries, target.dir ?? undefined);
    };
    const onKey = (k: KeyboardEvent) => {
      if (k.key === 'Escape') end(k);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', end);
    window.addEventListener('pointercancel', end);
    window.addEventListener('keydown', onKey);
  }

  const toolBtn =
    'inline-flex items-center gap-1 rounded-full border border-default px-2 py-1 text-xs ' +
    'font-medium text-muted transition hover:border-strong hover:bg-accent hover:text-accent-fg ' +
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus ' +
    'disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent ' +
    'disabled:hover:text-muted disabled:hover:border-default';
  const field =
    'w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none ' +
    'focus-visible:ring-2 focus-visible:ring-focus placeholder:text-faint';
</script>

<svelte:window onkeydown={onWindowKey} />

<!-- bg-surface fills behind the macOS traffic lights (no seam); the pt insets the
     panes below them. -->
<div
  bind:this={root}
  class="absolute inset-0 flex flex-col bg-surface pt-[var(--titlebar-h)] {active ? '' : 'hidden'}
    {drag ? 'cursor-grabbing' : ''}"
>
  {#if openError}
    <div class="flex flex-1 flex-col items-center justify-center gap-2 p-10 text-center">
      <p class="font-medium">{$t('sftp.openFailed', { host: session.hostName })}</p>
      <p class="max-w-md text-sm text-muted">{openError}</p>
    </div>
  {:else if !view}
    <div class="flex flex-1 items-center justify-center p-10 text-center">
      <p class="text-sm text-muted">{$t('sftp.connecting', { host: session.hostName })}</p>
    </div>
  {:else}
    <div class="grid min-h-0 flex-1 grid-cols-2 divide-x divide-default">
      <SftpPane
        title={$t('sftp.local')}
        side="local"
        pane={view.local}
        entries={localEntries}
        sort={$fileSort}
        showHidden={$showHidden}
        bind:filter={localFilter}
        bookmarked={localBookmarked}
        dropTarget={dropTarget?.side === 'local' ? dropTarget : undefined}
        onNavigate={(e) => navigate('local', e)}
        onOpenPath={(p) => openPath('local', p)}
        onUp={() => goUp('local')}
        onOpen={(e) => void openEntry('local', e)}
        onSelect={(p, mode) => select('local', p, mode)}
        onSelectAll={() => backendId != null && sftp.selectAll(backendId, 'local', localEntries)}
        onContextMenu={(e, x, y) => showMenu('local', e, x, y)}
        onRowPointerDown={(entry, e) => rowPointerDown('local', entry, e)}
        onSort={(key: SortKey) => fileSort.set(toggleSort($fileSort, key))}
        onToggleHidden={() => showHidden.set(!$showHidden)}
        onToggleBookmark={() => view && toggleBookmark('local', view.local.path)}
        onShowBookmarks={(x, y) => showBookmarks('local', x, y)}
        onKey={(k) => paneKey('local', k)}
      >
        {#snippet toolbar()}
          <button
            type="button"
            class={toolBtn}
            title={$t('sftp.uploadTitle')}
            disabled={localMarked.length === 0}
            onclick={() => upload()}
          >
            <Icon name="upload" size={13} />
            {$t('sftp.upload')}
          </button>
          <button
            type="button"
            class={toolBtn}
            title={$t('common.refresh')}
            aria-label={$t('sftp.refreshLocal')}
            onclick={() => refreshSide('local')}
          >
            <Icon name="refresh" size={13} />
          </button>
        {/snippet}
      </SftpPane>

      <SftpPane
        title={session.hostName}
        side="remote"
        pane={view.remote}
        entries={remoteEntries}
        sort={$fileSort}
        showHidden={$showHidden}
        bind:filter={remoteFilter}
        bookmarked={remoteBookmarked}
        dropTarget={dropTarget?.side === 'remote' ? dropTarget : undefined}
        onNavigate={(e) => navigate('remote', e)}
        onOpenPath={(p) => openPath('remote', p)}
        onUp={() => goUp('remote')}
        onOpen={(e) => void openEntry('remote', e)}
        onSelect={(p, mode) => select('remote', p, mode)}
        onSelectAll={() => backendId != null && sftp.selectAll(backendId, 'remote', remoteEntries)}
        onContextMenu={(e, x, y) => showMenu('remote', e, x, y)}
        onRowPointerDown={(entry, e) => rowPointerDown('remote', entry, e)}
        onSort={(key: SortKey) => fileSort.set(toggleSort($fileSort, key))}
        onToggleHidden={() => showHidden.set(!$showHidden)}
        onToggleBookmark={() => view && toggleBookmark(bookmarkKey('remote'), view.remote.path)}
        onShowBookmarks={(x, y) => showBookmarks('remote', x, y)}
        onKey={(k) => paneKey('remote', k)}
      >
        {#snippet toolbar()}
          <button
            type="button"
            class={toolBtn}
            title={$t('sftp.downloadTitle')}
            disabled={remoteMarked.length === 0}
            onclick={() => download()}
          >
            <Icon name="download" size={13} />
            {$t('sftp.download')}
          </button>
          <button type="button" class={toolBtn} title={$t('sftp.newFolder')} onclick={() => openPrompt('mkdir')}>
            <Icon name="plus" size={13} />
            {$t('sftp.folder')}
          </button>
          <button
            type="button"
            class={toolBtn}
            title={$t('sftp.renameTitle')}
            aria-label={$t('sftp.rename')}
            disabled={!singleRemoteMark}
            onclick={() => openPrompt('rename', 'remote')}
          >
            <Icon name="edit" size={13} />
          </button>
          <button
            type="button"
            class={toolBtn}
            title={$t('sftp.delete')}
            aria-label={$t('sftp.delete')}
            disabled={remoteMarked.length === 0}
            onclick={() => askDelete('remote', remoteMarked)}
          >
            <Icon name="trash" size={13} />
          </button>
          <button
            type="button"
            class="{toolBtn} {dockShown ? 'border-strong bg-surface-inset text-fg' : ''}"
            title={$t('dock.toggle')}
            aria-label={$t('dock.title')}
            aria-pressed={dockShown}
            onclick={toggleDock}
          >
            <Icon name="terminal" size={13} />
          </button>
          <button
            type="button"
            class={toolBtn}
            title={$t('common.refresh')}
            aria-label={$t('sftp.refreshRemote')}
            onclick={() => refreshSide('remote')}
          >
            <Icon name="refresh" size={13} />
          </button>
        {/snippet}
      </SftpPane>
    </div>

    {#if dockMounted}
      <section
        class="flex shrink-0 flex-col border-t border-default bg-surface {dockShown ? '' : 'hidden'}"
        style="height: {dockHeight}px"
        aria-label={$t('dock.title')}
      >
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="-mt-1 h-2 shrink-0 cursor-row-resize"
          title={$t('dock.resize')}
          onpointerdown={resizeDock}
        ></div>
        <div class="flex shrink-0 items-center gap-2 px-3 pb-1.5">
          <Icon name="terminal" size={13} />
          <span class="text-xs font-semibold uppercase tracking-[0.14em] text-muted">
            {$t('dock.title')}
          </span>
          <span class="min-w-0 truncate font-mono text-xs text-faint">{session.hostName}</span>
          <div class="ml-auto flex shrink-0 items-center gap-1">
            <button
              type="button"
              class={toolBtn}
              title={$t('dock.cdHere')}
              disabled={dockEnded}
              onclick={() => dockPane?.type(cdCommand(view?.remote.path ?? '/'))}
            >
              <Icon name="folder" size={13} />
              cd
            </button>
            <button type="button" class={toolBtn} aria-label={$t('dock.hide')} title={$t('dock.hide')} onclick={() => (dockShown = false)}>
              <Icon name="chevron-down" size={13} />
            </button>
            <button type="button" class={toolBtn} aria-label={$t('dock.close')} title={$t('dock.close')} onclick={closeDock}>
              <Icon name="close" size={13} />
            </button>
          </div>
        </div>
        <div class="relative min-h-0 flex-1 px-2 pb-2">
          {#key dockGeneration}
            <TerminalPane
              bind:this={dockPane}
              hostName={session.hostName}
              visible={active && dockShown}
              fade={false}
              initialInput={dockStartDir ? cdCommand(dockStartDir) : undefined}
              onOpened={dockOpened}
              onFailed={(message) => {
                lastError.set(message);
                dockEnded = true;
              }}
            />
          {/key}
          {#if dockEnded}
            <div class="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-surface text-sm">
              <p class="text-muted">{$t('dock.ended')}</p>
              <button type="button" class={toolBtn} onclick={() => openDock()}>
                <Icon name="refresh" size={13} />
                {$t('dock.restart')}
              </button>
            </div>
          {/if}
        </div>
      </section>
    {/if}

    {#if opening || working > 0 || note || (view.edit && !editNoteHidden)}
      <div class="shrink-0 border-t border-default px-4 py-2 text-xs" aria-live="polite">
        {#if working > 0}
          <span class="text-muted">{$t('fm.working')}</span>
        {:else if note}
          <span class="text-status-ok">{note}</span>
        {:else if opening}
          <span class="text-muted">{$t('sftp.opening', { name: opening })}</span>
        {:else if view.edit}
          {@const name = baseName(view.edit.path)}
          {#if view.edit.state === 'uploading'}
            <span class="text-muted">{$t('edit.uploading', { name })}</span>
          {:else if view.edit.state === 'synced'}
            <span class="text-status-ok">{$t('edit.synced', { name })}</span>
          {:else if view.edit.state === 'conflict'}
            <span class="text-status-warn">{$t('edit.conflictNote', { name })}</span>
          {:else if view.edit.state === 'modified'}
            <span class="text-muted">{$t('edit.modifiedNote', { name })}</span>
          {:else}
            <span class="text-status-crit" title={view.edit.error}>
              {$t('edit.failed', { name, error: view.edit.error ?? $t('edit.uploadFailed') })}
            </span>
          {/if}
        {/if}
      </div>
    {/if}

    {#if showQueue}
      <TransferPanel
        {queue}
        {preparing}
        onCancel={cancelTransfers}
        onRetry={retryTransfers}
        onDismiss={dismissTransfer}
        onClear={clearTransfers}
      />
    {:else if view.error}
      <div class="shrink-0 border-t border-default px-4 py-2 text-xs text-status-crit">
        {view.error}
      </div>
    {/if}
  {/if}
</div>

{#if drag}
  <div
    class="pointer-events-none fixed z-50 flex items-center gap-2 rounded-full border border-default
      bg-surface-raised px-3 py-1.5 text-xs text-fg shadow-soft"
    style="left: {drag.x + 14}px; top: {drag.y + 14}px"
  >
    <Icon name={drag.entries.length === 1 && drag.entries[0].isDir ? 'folder' : 'file'} size={13} />
    {#if drag.entries.length === 1}
      <span class="max-w-56 truncate font-mono">{drag.entries[0].name}</span>
    {:else}
      {$t('sftp.drag.items', { count: drag.entries.length })}
    {/if}
    {#if dropTarget}
      <span class="text-muted">→ {dropTarget.dir ? baseName(dropTarget.dir) : $t('sftp.drag.here')}</span>
    {/if}
  </div>
{/if}

{#if active && menu}
  <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => (menu = null)} />
{/if}

{#if active && conflictAsk}
  {#key conflictAsk.conflict.index}
    <ConflictDialog
      conflict={conflictAsk.conflict}
      remaining={conflictAsk.remaining}
      onAnswer={(action, applyToAll) => conflictAsk?.resolve({ action, applyToAll })}
      onCancel={() => conflictAsk?.resolve(null)}
    />
  {/key}
{:else if active && editPrompt}
  {@const path = editPrompt.path}
  {@const name = baseName(path)}
  {#if editPrompt.kind === 'modified'}
    <Modal label={$t('edit.modified.title')} onClose={() => void answerEditUpload(path, false)}>
      <header class="border-b border-default px-5 py-3.5">
        <h2 class="text-sm font-semibold">{$t('edit.modified.heading', { name })}</h2>
        <p class="mt-1 truncate font-mono text-xs text-muted" title={path}>{path}</p>
      </header>
      <p class="px-5 py-4 text-sm text-muted">{$t('edit.modified.body')}</p>
      <footer class="flex flex-wrap justify-end gap-2 border-t border-default px-5 py-3">
        <button
          type="button"
          class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg"
          onclick={() => void answerEditUpload(path, false)}
        >
          {$t('edit.modified.skip')}
        </button>
        <button
          type="button"
          class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg transition hover:opacity-90"
          onclick={() => void answerEditUpload(path, true)}
        >
          {$t('edit.modified.upload')}
        </button>
      </footer>
    </Modal>
  {:else}
    <Modal label={$t('edit.conflict.title')} onClose={() => {}}>
      <header class="border-b border-default px-5 py-3.5">
        <h2 class="text-sm font-semibold">{$t('edit.conflict.heading', { name })}</h2>
        <p class="mt-1 truncate font-mono text-xs text-muted" title={path}>{path}</p>
      </header>
      <p class="px-5 py-4 text-sm text-muted">{$t('edit.conflict.body')}</p>
      <footer class="flex flex-wrap justify-end gap-2 border-t border-default px-5 py-3">
        <button
          type="button"
          class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg"
          onclick={() => void answerEditConflict(path, false)}
        >
          {$t('edit.conflict.reload')}
        </button>
        <button
          type="button"
          class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg transition hover:opacity-90"
          onclick={() => void answerEditConflict(path, true)}
        >
          {$t('edit.conflict.overwrite')}
        </button>
      </footer>
    </Modal>
  {/if}
{/if}

{#if active && prompt}
  {@const titles = {
    mkdir: $t('sftp.newFolder'),
    rename: $t('sftp.renameOf', { name: prompt.targets[0]?.name ?? '' }),
    newFile: $t('fm.newFileTitle'),
    compress: $t('fm.compressTitle'),
    copyTo: $t('fm.copyTo'),
    moveTo: $t('fm.moveTo')
  }}
  {@const labels = {
    mkdir: $t('sftp.folderName'),
    rename: $t('sftp.newName'),
    newFile: $t('fm.fileName'),
    compress: $t('fm.archiveName'),
    copyTo: $t('fm.destination'),
    moveTo: $t('fm.destination')
  }}
  <Modal label={titles[prompt.kind]} onClose={() => (prompt = null)}>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        submitPrompt();
      }}
    >
      <header class="border-b border-default px-5 py-3.5">
        <h2 class="text-sm font-semibold">{titles[prompt.kind]}</h2>
        {#if prompt.targets.length && prompt.kind !== 'rename'}
          <p class="mt-1 truncate font-mono text-xs text-muted">
            {prompt.targets.map((t) => t.name).join(', ')}
          </p>
        {/if}
      </header>
      <div class="space-y-3 px-5 py-4">
        <!-- svelte-ignore a11y_autofocus -->
        <input
          autofocus
          bind:value={prompt.value}
          class="{field} {prompt.kind === 'copyTo' || prompt.kind === 'moveTo' ? 'font-mono' : ''}"
          placeholder={labels[prompt.kind]}
          aria-label={labels[prompt.kind]}
          spellcheck="false"
        />
        {#if prompt.kind === 'compress'}
          <div class="flex items-center gap-2 text-xs text-muted">
            {$t('fm.format')}
            {#each [['tarGz', '.tar.gz'], ['zip', '.zip']] as [fmt, label] (fmt)}
              <button
                type="button"
                class="rounded-lg px-2.5 py-1 font-mono transition {prompt.format === fmt
                  ? 'bg-accent text-accent-fg'
                  : 'bg-surface-inset text-muted hover:text-fg'}"
                aria-pressed={prompt.format === fmt}
                onclick={() => setFormat(fmt as ArchiveFormatDto)}
              >
                {label}
              </button>
            {/each}
          </div>
        {/if}
      </div>
      <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
        <button
          type="button"
          class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg"
          onclick={() => (prompt = null)}
        >
          {$t('common.cancel')}
        </button>
        <button
          type="submit"
          class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg transition hover:opacity-90 disabled:opacity-50"
          disabled={!prompt.value.trim()}
        >
          {prompt.kind === 'rename' ? $t('sftp.rename') : prompt.kind === 'mkdir' || prompt.kind === 'newFile' ? $t('sftp.create') : $t('fm.apply')}
        </button>
      </footer>
    </form>
  </Modal>
{/if}

{#if active && confirmDelete}
  {@const doomed = confirmDelete}
  <Modal label={$t('fm.deleteTitle')} onClose={() => (confirmDelete = null)}>
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="text-sm font-semibold">{$t('fm.deleteTitle')}</h2>
      <p class="mt-1 truncate font-mono text-xs text-muted">{doomed.map((e) => e.name).join(', ')}</p>
    </header>
    <p class="px-5 py-4 text-sm text-muted">
      {$t('fm.deleteBody', { count: doomed.length, host: session.hostName })}
    </p>
    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <button
        type="button"
        class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg"
        onclick={() => (confirmDelete = null)}
      >
        {$t('common.cancel')}
      </button>
      <button
        type="button"
        class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg transition hover:opacity-90"
        onclick={() => {
          // Read before closing: `doomed` follows `confirmDelete`.
          const paths = doomed.map((e) => e.path);
          confirmDelete = null;
          void runRemote({ kind: 'delete', paths });
        }}
      >
        {$t('fm.deleteConfirm')}
      </button>
    </footer>
  </Modal>
{/if}

{#if active && confirmExtract && view}
  {@const archive = confirmExtract}
  {@const dest = view.remote.path}
  <Modal label={$t('fm.extract')} onClose={() => (confirmExtract = null)}>
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="text-sm font-semibold">{$t('fm.extract')}</h2>
      <p class="mt-1 truncate font-mono text-xs text-muted">{archive.name}</p>
    </header>
    <p class="px-5 py-4 text-sm text-muted">{$t('fm.extractConfirm', { dir: dest })}</p>
    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <button
        type="button"
        class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg"
        onclick={() => (confirmExtract = null)}
      >
        {$t('common.cancel')}
      </button>
      <button
        type="button"
        class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg transition hover:opacity-90"
        onclick={() => {
          const op: RemoteFsOpDto = { kind: 'extract', archive: archive.path, dest };
          confirmExtract = null;
          void runRemote(op);
        }}
      >
        {$t('fm.extract')}
      </button>
    </footer>
  </Modal>
{/if}

{#if active && permsFor}
  {@const targets = permsFor}
  <PermissionsDialog
    names={targets.map((e) => e.name)}
    initial={targets[0]?.permissions ?? 0o644}
    hasFolders={targets.some((e) => e.isDir)}
    onCancel={() => (permsFor = null)}
    onApply={(mode, recursive) => {
      const paths = targets.map((e) => e.path);
      permsFor = null;
      void runRemote({ kind: 'chmod', paths, mode, recursive });
    }}
  />
{/if}

{#if active && view?.preview}
  <Modal label={$t('sftp.preview')} onClose={closePreview}>
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="truncate font-mono text-xs text-muted" title={view.preview.path}>
        {view.preview.path}
      </h2>
    </header>
    <div class="min-h-0 flex-1 overflow-auto px-5 py-4">
      {#if view.preview.content.length === 0}
        <p class="text-sm text-faint">{$t('sftp.emptyFile')}</p>
      {:else}
        <pre class="select-text whitespace-pre-wrap break-words font-mono text-xs text-fg">{view.preview
            .content}</pre>
      {/if}
    </div>
    <footer class="flex justify-end border-t border-default px-5 py-3">
      <button
        type="button"
        class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg"
        onclick={closePreview}
      >
        {$t('common.close')}
      </button>
    </footer>
  </Modal>
{/if}
