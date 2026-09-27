import { writable } from 'svelte/store';

// The one entity Content shows (tech-gui.md §2, §3.5): a selector screen or one
// session, never both. This store is the sole writer of "what's active", so the
// exactly-one-active invariant holds by construction — a new active value replaces
// the previous one whatever its kind. Switching selectors deactivates any session;
// activating a session deactivates the selectors.
export type ActiveEntity =
  | { kind: 'dashboard' }
  | { kind: 'snippets' }
  | { kind: 'keys' }
  | { kind: 'settings' }
  | { kind: 'session'; id: number };

function createActiveEntity() {
  const { subscribe, set } = writable<ActiveEntity>({ kind: 'dashboard' });
  return {
    subscribe,
    selectDashboard: () => set({ kind: 'dashboard' }),
    selectSnippets: () => set({ kind: 'snippets' }),
    selectKeys: () => set({ kind: 'keys' }),
    selectSettings: () => set({ kind: 'settings' }),
    activateSession: (id: number) => set({ kind: 'session', id })
  };
}

export const activeEntity = createActiveEntity();
