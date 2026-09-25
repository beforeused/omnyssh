import { describe, expect, it } from 'vitest';
import { get } from 'svelte/store';
import type { TransferItemDto, TransferUpdateDto } from '$lib/bindings';
import {
  transfers,
  addItems,
  applyUpdates,
  summarize,
  clearFinished,
  parentDir,
  formatDuration,
  EMPTY_QUEUE
} from './transfers';

// The background transfer queue's numbers — per-file state, smoothed speed, overall
// progress, ETA, and which folders received files — are pure reducers (§6.4).

function item(id: number, size: number, direction: 'upload' | 'download' = 'upload'): TransferItemDto {
  return {
    id,
    direction,
    name: `f${id}.bin`,
    local: `/home/me/f${id}.bin`,
    remote: `/srv/app/f${id}.bin`,
    size
  };
}

function update(id: number, state: TransferUpdateDto['state'], done: number, total: number, error?: string): TransferUpdateDto {
  return { id, state, done, total, error: error ?? null };
}

describe('transfer queue reducers', () => {
  it('lists committed transfers as queued, once', () => {
    let q = addItems(EMPTY_QUEUE, [item(1, 100), item(2, 200)]);
    q = addItems(q, [item(2, 200), item(3, 300)]);
    expect(q.items.map((i) => [i.id, i.state, i.total])).toEqual([
      [1, 'queued', 100],
      [2, 'queued', 200],
      [3, 'queued', 300]
    ]);
  });

  it('smooths speed from successive samples and zeroes it when settled', () => {
    let q = addItems(EMPTY_QUEUE, [item(1, 10_000_000)]);
    q = applyUpdates(q, [update(1, 'running', 0, 10_000_000)], 1_000);
    expect(q.items[0].speed).toBe(0);
    q = applyUpdates(q, [update(1, 'running', 1_000_000, 10_000_000)], 2_000);
    expect(q.items[0].speed).toBe(1_000_000);
    q = applyUpdates(q, [update(1, 'running', 4_000_000, 10_000_000)], 3_000);
    // 3 MB/s instant, blended with the 1 MB/s estimate.
    expect(q.items[0].speed).toBeCloseTo(1_600_000);
    q = applyUpdates(q, [update(1, 'done', 10_000_000, 10_000_000)], 4_000);
    expect(q.items[0].speed).toBe(0);
    expect(q.items[0].state).toBe('done');
  });

  it('records the folder a finished file landed in, per side', () => {
    let q = addItems(EMPTY_QUEUE, [item(1, 5), item(2, 5, 'download')]);
    q = applyUpdates(q, [update(1, 'done', 5, 5), update(2, 'done', 5, 5)], 1);
    expect(q.landed).toEqual({ local: ['/home/me'], remote: ['/srv/app'] });
    // Re-reporting a finished transfer does not land it twice.
    const again = applyUpdates({ ...q, landed: { local: [], remote: [] } }, [update(1, 'done', 5, 5)], 2);
    expect(again.landed.remote).toEqual([]);
  });

  it('ignores updates for transfers it does not list (the editor sync’s own)', () => {
    const q = addItems(EMPTY_QUEUE, [item(1, 5)]);
    expect(applyUpdates(q, [update(99, 'running', 1, 5)], 1)).toBe(q);
  });

  it('restarts the estimate when a retry reuses the id', () => {
    let q = addItems(EMPTY_QUEUE, [item(1, 100)]);
    q = applyUpdates(q, [update(1, 'running', 10, 100)], 1_000);
    q = applyUpdates(q, [update(1, 'running', 60, 100)], 2_000);
    q = applyUpdates(q, [update(1, 'failed', 60, 100, 'connection reset')], 3_000);
    expect(q.items[0].error).toBe('connection reset');
    q = applyUpdates(q, [update(1, 'queued', 0, 100)], 4_000);
    q = applyUpdates(q, [update(1, 'running', 5, 100)], 5_000);
    expect(q.items[0].error).toBeUndefined();
    expect(q.items[0].speed).toBe(0);
  });

  it('summarizes active work, throughput and time left', () => {
    let q = addItems(EMPTY_QUEUE, [item(1, 1000), item(2, 1000), item(3, 1000), item(4, 1000)]);
    q = applyUpdates(q, [update(1, 'running', 0, 1000), update(2, 'done', 1000, 1000)], 0);
    q = applyUpdates(q, [update(1, 'running', 500, 1000), update(3, 'failed', 10, 1000, 'x')], 1_000);
    const s = summarize(q);
    expect(s.active).toBe(2); // #1 running, #4 queued
    expect(s.failed).toBe(1);
    expect(s.finished).toBe(1);
    // Failed work is out of the overall bar: 500 + 1000 + 0 of 3000.
    expect([s.done, s.total]).toEqual([1500, 3000]);
    expect(s.speed).toBe(500);
    // 500 left on #1 + 1000 on #4, at 500 B/s.
    expect(s.eta).toBe(3);
  });

  it('clears settled transfers, failed ones only on request', () => {
    let q = addItems(EMPTY_QUEUE, [item(1, 1), item(2, 1), item(3, 1), item(4, 1)]);
    q = applyUpdates(
      q,
      [update(1, 'done', 1, 1), update(2, 'cancelled', 0, 1), update(3, 'failed', 0, 1, 'x')],
      0
    );
    const soft = clearFinished(q);
    expect(soft.removed).toEqual([1, 2]);
    expect(soft.queue.items.map((i) => i.id)).toEqual([3, 4]);
    expect(clearFinished(q, true).removed).toEqual([1, 2, 3]);
  });

  it('parentDir handles POSIX and Windows paths', () => {
    expect(parentDir('/srv/app/f.txt')).toBe('/srv/app');
    expect(parentDir('/f.txt')).toBe('/');
    expect(parentDir('C:\\Users\\me\\f.txt')).toBe('C:\\Users\\me');
    expect(parentDir('C:\\f.txt')).toBe('C:\\');
  });

  it('formatDuration reads naturally', () => {
    expect(formatDuration(0.2)).toBe('1 s');
    expect(formatDuration(42)).toBe('42 s');
    expect(formatDuration(600)).toBe('10 min');
    expect(formatDuration(3900)).toBe('1 h 5 min');
  });
});

describe('transfers store', () => {
  it('keeps tabs apart and hands landed folders over once', () => {
    transfers.add(1, [item(1, 5)]);
    transfers.add(2, [item(1, 7, 'download')]);
    transfers.apply(1, [update(1, 'done', 5, 5)]);
    expect(get(transfers).get(2)?.items[0].state).toBe('queued');
    expect(transfers.takeLanded(1)).toEqual({ local: [], remote: ['/srv/app'] });
    expect(transfers.takeLanded(1)).toEqual({ local: [], remote: [] });
    transfers.remove(1);
    transfers.remove(2);
    expect(get(transfers).size).toBe(0);
  });

  it('drops updates for a closed tab', () => {
    transfers.apply(42, [update(1, 'running', 1, 2)]);
    expect(get(transfers).has(42)).toBe(false);
  });
});
