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
  import SftpPane, { type SelectMode } from './SftpPane.svelte';
  import TransferPanel from './TransferPanel.svelte';
  import ConflictDialog from './ConflictDialog.svelte';
  import TerminalPane from './TerminalPane.svelte';
  import type {
    ConflictActionDto,
    ConflictResolutionDto,
    FileEntryDto,
    TransferConflictDto,
    TransferDirectionDto
  } from '$lib/bindings';
  import { sessions, type Session } from '$lib/stores/sessions';
  import { registerCloseGuard } from '$lib/stores/navigation';
  import { sftp, markedEntries, type PaneSide } from '$lib/stores/sftp';
  import { transfers, EMPTY_QUEUE, isActive } from '$lib/stores/transfers';
  import { editor } from '$lib/stores/settings';
  import { lastError } from '$lib/stores/notifications';
  import { onTerminalExit } from '$lib/ipc/router';
  import { t } from '$lib/i18n';
  import {
    sftpOpen,
    sftpList,
    sftpClose,
    sftpMkdir,
    sftpRename,
    sftpDelete,
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
    editConfirmUpload
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
  let prompt = $state<{ kind: 'mkdir' | 'rename'; value: string; target?: FileEntryDto } | null>(
    null
  );

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
  const editPrompt = $derived(view?.editPrompts[0]);

  function errMsg(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  function joinRemote(dir: string, name: string): string {
    return dir.endsWith('/') ? `${dir}${name}` : `${dir}/${name}`;
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
    else if (mode === 'range') sftp.selectRange(id, side, path);
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

  function remove(entries: FileEntryDto[] = remoteMarked): void {
    const id = backendId;
    if (id == null) return;
    enqueue(
      ...entries.map((entry) => () => {
        sftp.pushOp(id, { kind: 'delete', name: entry.name, refresh: 'remote' });
        void sftpDelete(id, entry.path).catch(onDispatchError(id));
      })
    );
  }

  function openPrompt(kind: 'mkdir' | 'rename', target: FileEntryDto | undefined = singleRemoteMark): void {
    if (kind === 'rename' && target) {
      prompt = { kind, value: target.name, target };
    } else if (kind === 'mkdir') {
      prompt = { kind, value: '' };
    }
  }

  function submitPrompt(): void {
    const id = backendId;
    if (id == null || !view || !prompt) return;
    const value = prompt.value.trim();
    if (!value) return;
    const dir = view.remote.path;
    if (prompt.kind === 'mkdir') {
      enqueue(() => {
        sftp.pushOp(id, { kind: 'mkdir', refresh: 'remote' });
        void sftpMkdir(id, joinRemote(dir, value)).catch(onDispatchError(id));
      });
    } else if (prompt.target) {
      const from = prompt.target.path;
      enqueue(() => {
        sftp.pushOp(id, { kind: 'rename', refresh: 'remote' });
        void sftpRename(id, from, joinRemote(dir, value)).catch(onDispatchError(id));
      });
    }
    prompt = null;
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
    const items: MenuItem[] = [];
    if (entry && !many) {
      if (entry.isDir) {
        items.push({ label: $t('sftp.menu.open'), icon: 'folder', run: () => navigate(side, entry) });
      } else {
        items.push({
          label: side === 'remote' ? $t('sftp.menu.openInEditor') : $t('sftp.menu.open'),
          icon: 'external',
          run: () => void openEntry(side, entry)
        });
        items.push({ label: $t('sftp.menu.quickLook'), icon: 'eye', run: () => void preview(side, entry) });
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
    if (side === 'remote') {
      if (entry && !many) {
        items.push({ label: $t('sftp.menu.rename'), icon: 'edit', run: () => openPrompt('rename', entry) });
      }
      if (n) {
        items.push({
          label: n > 1 ? $t('sftp.menu.deleteN', { count: n }) : $t('sftp.menu.delete'),
          icon: 'trash',
          danger: true,
          run: () => remove(targets)
        });
      }
      if (!entry) items.push({ label: $t('sftp.menu.newFolder'), icon: 'plus', run: () => openPrompt('mkdir') });
      const dir = entry?.isDir && !many ? entry.path : view.remote.path;
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
        dropTarget={dropTarget?.side === 'local' ? dropTarget : undefined}
        onNavigate={(e) => navigate('local', e)}
        onOpen={(e) => void openEntry('local', e)}
        onSelect={(p, mode) => select('local', p, mode)}
        onSelectAll={() => backendId != null && sftp.selectAll(backendId, 'local')}
        onContextMenu={(e, x, y) => showMenu('local', e, x, y)}
        onRowPointerDown={(entry, e) => rowPointerDown('local', entry, e)}
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
        dropTarget={dropTarget?.side === 'remote' ? dropTarget : undefined}
        onNavigate={(e) => navigate('remote', e)}
        onOpen={(e) => void openEntry('remote', e)}
        onSelect={(p, mode) => select('remote', p, mode)}
        onSelectAll={() => backendId != null && sftp.selectAll(backendId, 'remote')}
        onContextMenu={(e, x, y) => showMenu('remote', e, x, y)}
        onRowPointerDown={(entry, e) => rowPointerDown('remote', entry, e)}
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
            onclick={() => openPrompt('rename')}
          >
            <Icon name="edit" size={13} />
          </button>
          <button
            type="button"
            class={toolBtn}
            title={$t('sftp.delete')}
            aria-label={$t('sftp.delete')}
            disabled={remoteMarked.length === 0}
            onclick={() => remove()}
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

    {#if opening || (view.edit && !editNoteHidden)}
      <div class="shrink-0 border-t border-default px-4 py-2 text-xs" aria-live="polite">
        {#if opening}
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
  <Modal label={prompt.kind === 'mkdir' ? $t('sftp.newFolder') : $t('sftp.rename')} onClose={() => (prompt = null)}>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        submitPrompt();
      }}
    >
      <header class="border-b border-default px-5 py-3.5">
        <h2 class="text-sm font-semibold">
          {prompt.kind === 'mkdir' ? $t('sftp.newFolder') : $t('sftp.renameOf', { name: prompt.target?.name ?? '' })}
        </h2>
      </header>
      <div class="px-5 py-4">
        <!-- svelte-ignore a11y_autofocus -->
        <input
          autofocus
          bind:value={prompt.value}
          class={field}
          placeholder={prompt.kind === 'mkdir' ? $t('sftp.folderName') : $t('sftp.newName')}
          aria-label={prompt.kind === 'mkdir' ? $t('sftp.folderName') : $t('sftp.newName')}
        />
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
          {prompt.kind === 'mkdir' ? $t('sftp.create') : $t('sftp.rename')}
        </button>
      </footer>
    </form>
  </Modal>
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
