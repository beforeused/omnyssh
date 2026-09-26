import { writable } from 'svelte/store';
import type {
  TransferDirectionDto,
  TransferItemDto,
  TransferStateDto,
  TransferUpdateDto
} from '$lib/bindings';
import { tr } from '$lib/i18n';

// Per-SFTP-tab transfer queue, keyed by the backend session id like the sftp store.
// Items arrive from `transfer_commit`; the engine's batched `transfers-updated`
// events then move their progress and state. Everything is a pure reducer so the
// queue panel's numbers (speed, ETA, overall progress) are unit-testable.

export interface TransferItem {
  id: number;
  direction: TransferDirectionDto;
  name: string;
  local: string;
  remote: string;
  state: TransferStateDto;
  done: number;
  total: number;
  error?: string;
  /** Smoothed throughput in bytes/s while running, 0 otherwise. */
  speed: number;
  /** When the last progress sample landed (ms), for the speed estimate. */
  sampledAt?: number;
}

export interface TransferQueue {
  items: TransferItem[];
  /** Folders (by side) that received finished files since the view last looked. */
  landed: { local: string[]; remote: string[] };
}

export const EMPTY_QUEUE: TransferQueue = { items: [], landed: { local: [], remote: [] } };

/** Weight of the newest sample in the speed estimate. */
const SPEED_ALPHA = 0.3;

export function isActive(state: TransferStateDto): boolean {
  return state === 'queued' || state === 'running' || state === 'reconnecting';
}

/** Parent folder of a local or remote path ('/' for a top-level entry). */
export function parentDir(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  if (cut < 0) return path;
  if (cut === 0) return path.slice(0, 1);
  // Keep a Windows drive root ("C:\") intact.
  if (/^[A-Za-z]:$/.test(path.slice(0, cut))) return path.slice(0, cut + 1);
  return path.slice(0, cut);
}

/** Append freshly committed transfers (ignoring ids already listed). */
export function addItems(queue: TransferQueue, dtos: TransferItemDto[]): TransferQueue {
  const known = new Set(queue.items.map((i) => i.id));
  const fresh: TransferItem[] = dtos
    .filter((d) => !known.has(d.id))
    .map((d) => ({
      id: d.id,
      direction: d.direction,
      name: d.name,
      local: d.local,
      remote: d.remote,
      state: 'queued',
      done: 0,
      total: d.size,
      speed: 0
    }));
  return fresh.length ? { ...queue, items: [...queue.items, ...fresh] } : queue;
}

/** Fold a batch of engine updates in at time `now` (ms). Updates for ids this
 *  queue does not list (the editor sync's own transfers) are ignored. */
export function applyUpdates(
  queue: TransferQueue,
  updates: TransferUpdateDto[],
  now: number
): TransferQueue {
  if (!updates.length) return queue;
  const byId = new Map(updates.map((u) => [u.id, u]));
  const landedLocal = new Set(queue.landed.local);
  const landedRemote = new Set(queue.landed.remote);
  let touched = false;
  const items = queue.items.map((item) => {
    const u = byId.get(item.id);
    if (!u) return item;
    touched = true;
    // A retry restarts the transfer under the same id: start the estimate over.
    const restarted = u.done < item.done;
    let speed = 0;
    if (u.state === 'running' && item.sampledAt !== undefined && !restarted) {
      const dt = (now - item.sampledAt) / 1000;
      const instant = dt > 0 ? (u.done - item.done) / dt : item.speed;
      speed = item.speed > 0 ? item.speed + SPEED_ALPHA * (instant - item.speed) : instant;
    }
    if (u.state === 'done' && item.state !== 'done') {
      if (item.direction === 'upload') landedRemote.add(parentDir(item.remote));
      else landedLocal.add(parentDir(item.local));
    }
    return {
      ...item,
      state: u.state,
      done: u.done,
      total: u.total,
      error: u.error ?? undefined,
      speed: Math.max(0, speed),
      sampledAt: u.state === 'running' ? now : undefined
    };
  });
  if (!touched) return queue;
  return {
    items,
    landed: { local: [...landedLocal], remote: [...landedRemote] }
  };
}

export interface QueueSummary {
  active: number;
  failed: number;
  finished: number;
  /** Bytes moved / to move across everything not cancelled or failed. */
  done: number;
  total: number;
  /** Combined throughput of running transfers, bytes/s. */
  speed: number;
  /** Seconds until the active transfers finish at the current speed. */
  eta?: number;
}

export function summarize(queue: TransferQueue): QueueSummary {
  let active = 0;
  let failed = 0;
  let finished = 0;
  let done = 0;
  let total = 0;
  let speed = 0;
  let remaining = 0;
  for (const item of queue.items) {
    if (isActive(item.state)) {
      active += 1;
      remaining += Math.max(0, item.total - item.done);
    }
    if (item.state === 'failed') failed += 1;
    if (item.state === 'done' || item.state === 'cancelled') finished += 1;
    if (item.state === 'running') speed += item.speed;
    if (item.state !== 'cancelled' && item.state !== 'failed') {
      done += Math.min(item.done, item.total);
      total += item.total;
    }
  }
  const eta = active > 0 && speed > 0 ? remaining / speed : undefined;
  return { active, failed, finished, done, total, speed, eta };
}

/** Drop settled transfers (done/cancelled, and failed too when `includeFailed`). */
export function clearFinished(
  queue: TransferQueue,
  includeFailed = false
): { queue: TransferQueue; removed: number[] } {
  const removed: number[] = [];
  const items = queue.items.filter((i) => {
    const drop =
      i.state === 'done' || i.state === 'cancelled' || (includeFailed && i.state === 'failed');
    if (drop) removed.push(i.id);
    return !drop;
  });
  return { queue: { ...queue, items }, removed };
}

/** "12 s", "4 min", "1 h 5 min" — in the UI language. */
export function formatDuration(seconds: number): string {
  const s = Math.max(1, Math.round(seconds));
  if (s < 60) return tr('duration.s', { n: s });
  const m = Math.round(s / 60);
  if (m < 60) return tr('duration.m', { n: m });
  const h = Math.floor(m / 60);
  return tr('duration.hm', { h, m: m % 60 });
}

function createTransfers() {
  const { subscribe, update } = writable<Map<number, TransferQueue>>(new Map());

  function mut(id: number, fn: (q: TransferQueue) => TransferQueue): void {
    update((m) => {
      const current = m.get(id) ?? EMPTY_QUEUE;
      const next = fn(current);
      if (next === current && m.has(id)) return m;
      return new Map(m).set(id, next);
    });
  }

  return {
    subscribe,
    add(id: number, items: TransferItemDto[]): void {
      mut(id, (q) => addItems(q, items));
    },
    apply(id: number, updates: TransferUpdateDto[], now = Date.now()): void {
      update((m) => {
        const current = m.get(id);
        if (!current) return m;
        const next = applyUpdates(current, updates, now);
        return next === current ? m : new Map(m).set(id, next);
      });
    },
    /** Hand the folders that received files to the view (and forget them). */
    takeLanded(id: number): { local: string[]; remote: string[] } {
      let landed = { local: [] as string[], remote: [] as string[] };
      update((m) => {
        const current = m.get(id);
        if (!current || (!current.landed.local.length && !current.landed.remote.length)) return m;
        landed = current.landed;
        return new Map(m).set(id, { ...current, landed: { local: [], remote: [] } });
      });
      return landed;
    },
    clearFinished(id: number, includeFailed = false): number[] {
      let removed: number[] = [];
      mut(id, (q) => {
        const out = clearFinished(q, includeFailed);
        removed = out.removed;
        return out.queue;
      });
      return removed;
    },
    dismiss(id: number, transferId: number): void {
      mut(id, (q) => ({ ...q, items: q.items.filter((i) => i.id !== transferId) }));
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

export const transfers = createTransfers();
