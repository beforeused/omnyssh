import { get, writable } from 'svelte/store';
import type { EditorDto } from '$lib/bindings';

// The metric auto-refresh interval, in seconds (tech-gui.md §4.3). A UI preference —
// persisted via tauri-plugin-store with a localStorage mirror, exactly like the sidebar
// collapse / theme (§4.2, "UI prefs use tauri-plugin-store directly, no bespoke Rust
// command"). The frontend drives an immediate `refresh_metrics` on this cadence; the
// backend keeps its own baseline poll, so this is a floor on how *fresh* the dashboard
// stays, not a throttle.
const LOCAL_KEY = 'omnyssh-refresh-interval';
const STORE_FILE = 'settings.json';
const STORE_KEY = 'refreshInterval';

/** The intervals the settings screen offers, in seconds. */
export const REFRESH_OPTIONS = [10, 30, 60, 120, 300] as const;
const DEFAULT = 30;

/** Coerce any stored/typed value to a sane positive interval (seconds). */
export function clampInterval(value: unknown): number {
  const n = typeof value === 'number' ? value : Number(value);
  return Number.isFinite(n) && n >= 1 ? Math.round(n) : DEFAULT;
}

function mirrored(): number {
  try {
    const raw = localStorage.getItem(LOCAL_KEY);
    return raw == null ? DEFAULT : clampInterval(raw);
  } catch {
    return DEFAULT;
  }
}

function mirrorLocal(seconds: number): void {
  try {
    localStorage.setItem(LOCAL_KEY, String(seconds));
  } catch {
    // localStorage unavailable (hardened webview): the store copy is canonical.
  }
}

async function persistStore(seconds: number): Promise<void> {
  try {
    const { load } = await import('@tauri-apps/plugin-store');
    const store = await load(STORE_FILE);
    await store.set(STORE_KEY, seconds);
    await store.save();
  } catch {
    // Not under Tauri (tests, vite preview): the localStorage mirror suffices.
  }
}

function createRefreshInterval() {
  const initial = mirrored();
  const { subscribe, set: setStore } = writable<number>(initial);
  let current = initial;
  let interacted = false;

  function apply(seconds: number, user: boolean): void {
    const value = clampInterval(seconds);
    current = value;
    setStore(value);
    mirrorLocal(value);
    if (user) {
      interacted = true;
      void persistStore(value);
    }
  }

  return {
    subscribe,
    set: (seconds: number) => apply(seconds, true),
    /** Reconcile with the canonical tauri-plugin-store value once Tauri is reachable. */
    async hydrate(): Promise<void> {
      try {
        const { load } = await import('@tauri-apps/plugin-store');
        const store = await load(STORE_FILE);
        const saved = await store.get<number>(STORE_KEY);
        if (!interacted && typeof saved === 'number') apply(saved, false);
      } catch {
        // Store unreachable: keep the mirrored value.
      }
    }
  };
}

export const refreshInterval = createRefreshInterval();

/** Drive `refresh` on the current interval, re-arming whenever the interval changes.
 *  Returns a disposer. Kept free of the ipc layer so it stays unit-testable — the
 *  layout passes in the actual `refresh_metrics` call. */
export function driveMetricsRefresh(
  refresh: () => void,
  setInterval_: typeof setInterval = setInterval,
  clearInterval_: typeof clearInterval = clearInterval
): () => void {
  let timer: ReturnType<typeof setInterval> | undefined;
  const unsub = refreshInterval.subscribe((seconds) => {
    if (timer !== undefined) clearInterval_(timer);
    timer = setInterval_(refresh, clampInterval(seconds) * 1000);
  });
  return () => {
    unsub();
    if (timer !== undefined) clearInterval_(timer);
  };
}

// ---------------------------------------------------------------------------
// Files: the editor that opens files, and how many parallel connections a host's
// transfers may use. Persisted like the refresh interval (localStorage mirror for
// the first paint, tauri-plugin-store as the canonical copy).
// ---------------------------------------------------------------------------

/** A persisted UI preference. `coerce` sanitises anything read back from storage. */
export function createPref<T>(localKey: string, storeKey: string, fallback: T, coerce: (raw: unknown) => T) {
  function mirrored(): T {
    try {
      const raw = localStorage.getItem(localKey);
      return raw == null ? fallback : coerce(JSON.parse(raw));
    } catch {
      return fallback;
    }
  }

  const initial = mirrored();
  const store = writable<T>(initial);
  const { subscribe, set: setStore } = store;
  let interacted = false;

  function apply(value: T, user: boolean): void {
    const next = coerce(value);
    setStore(next);
    try {
      localStorage.setItem(localKey, JSON.stringify(next));
    } catch {
      // localStorage unavailable: the store copy is canonical.
    }
    if (user) {
      interacted = true;
      void (async () => {
        try {
          const { load } = await import('@tauri-apps/plugin-store');
          const store = await load(STORE_FILE);
          await store.set(storeKey, next);
          await store.save();
        } catch {
          // Not under Tauri: the mirror suffices.
        }
      })();
    }
  }

  return {
    subscribe,
    set: (value: T) => apply(value, true),
    update: (fn: (current: T) => T) => apply(fn(get(store)), true),
    async hydrate(): Promise<void> {
      try {
        const { load } = await import('@tauri-apps/plugin-store');
        const store = await load(STORE_FILE);
        const saved = await store.get<unknown>(storeKey);
        if (!interacted && saved !== undefined && saved !== null) apply(coerce(saved), false);
      } catch {
        // Store unreachable: keep the mirrored value.
      }
    }
  };
}

/** Which program opens files — the same union the backend's `EditorDto` accepts. */
export type EditorChoice = EditorDto;

const SYSTEM_EDITOR: EditorChoice = { kind: 'system' };

/** Coerce a stored editor choice, falling back to the OS default. */
export function coerceEditor(raw: unknown): EditorChoice {
  if (!raw || typeof raw !== 'object') return SYSTEM_EDITOR;
  const r = raw as Record<string, unknown>;
  if (r.kind === 'app' && typeof r.path === 'string' && r.path) {
    return { kind: 'app', path: r.path, name: typeof r.name === 'string' && r.name ? r.name : r.path };
  }
  if (r.kind === 'command' && typeof r.command === 'string' && r.command.trim()) {
    return { kind: 'command', command: r.command };
  }
  return SYSTEM_EDITOR;
}

export const editor = createPref<EditorChoice>('omnyssh-editor', 'editor', SYSTEM_EDITOR, coerceEditor);

/** Parallel connections per host the settings screen offers. */
export const STREAM_OPTIONS = [1, 2, 4, 6, 8] as const;
const DEFAULT_STREAMS = 4;

export function clampStreams(raw: unknown): number {
  const n = typeof raw === 'number' ? raw : Number(raw);
  return Number.isFinite(n) ? Math.min(8, Math.max(1, Math.round(n))) : DEFAULT_STREAMS;
}

export const transferStreams = createPref<number>(
  'omnyssh-transfer-streams',
  'transferStreams',
  DEFAULT_STREAMS,
  clampStreams
);

// ---------------------------------------------------------------------------
// File manager: hidden files, sort order, bookmarks
// ---------------------------------------------------------------------------

export const showHidden = createPref<boolean>('omnyssh-show-hidden', 'showHidden', true, (raw) =>
  typeof raw === 'boolean' ? raw : true
);

export const fileSort = createPref<{ key: 'name' | 'size' | 'modified'; dir: 'asc' | 'desc' }>(
  'omnyssh-file-sort',
  'fileSort',
  { key: 'name', dir: 'asc' },
  (raw) => {
    const r = (raw ?? {}) as Record<string, unknown>;
    const key = r.key === 'size' || r.key === 'modified' ? r.key : 'name';
    const dir = r.dir === 'desc' ? 'desc' : 'asc';
    return { key, dir };
  }
);

/** Bookmarked folders, keyed `local` or `remote:<host>`. */
export const bookmarks = createPref<Record<string, string[]>>(
  'omnyssh-bookmarks',
  'bookmarks',
  {},
  (raw) => {
    if (!raw || typeof raw !== 'object') return {};
    const out: Record<string, string[]> = {};
    for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
      if (Array.isArray(v)) out[k] = v.filter((p): p is string => typeof p === 'string').slice(0, 50);
    }
    return out;
  }
);

export function toggleBookmark(key: string, path: string): void {
  bookmarks.update((all) => {
    const list = all[key] ?? [];
    const next = list.includes(path) ? list.filter((p) => p !== path) : [...list, path];
    return { ...all, [key]: next };
  });
}
