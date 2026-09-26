import { get } from 'svelte/store';
import { activeEntity } from './activeEntity';
import { sessions, type Session, type SessionKind, type SpawnOptions } from './sessions';

// Composed navigation actions that keep the sessions list and the active entity in
// step (tech-gui.md §2). A spawn appends a session and makes it active (both spawn
// paths do this); closing the active session falls back to the Dashboard so Content
// is never left pointing at a closed tab.
export function spawnSession(kind: SessionKind, hostName: string, options?: SpawnOptions): Session {
  const session = sessions.spawn(kind, hostName, options);
  activeEntity.activateSession(session.id);
  return session;
}

// A tab may veto its own close (an SFTP tab with transfers still running asks
// first). Guards are registered by the tab and answer asynchronously.
type CloseGuard = () => boolean | Promise<boolean>;
const closeGuards = new Map<number, CloseGuard>();

/** Register a guard consulted before session `id` closes; returns the disposer. */
export function registerCloseGuard(id: number, guard: CloseGuard): () => void {
  closeGuards.set(id, guard);
  return () => {
    if (closeGuards.get(id) === guard) closeGuards.delete(id);
  };
}

function forceClose(id: number): void {
  closeGuards.delete(id);
  const active = get(activeEntity);
  if (active.kind === 'session' && active.id === id) {
    activeEntity.selectDashboard();
  }
  sessions.close(id);
}

/** Close a session tab. Without a guard this is immediate; a guarded tab closes
 *  once its guard agrees. */
export function closeSession(id: number): void {
  const guard = closeGuards.get(id);
  if (!guard) {
    forceClose(id);
    return;
  }
  void Promise.resolve()
    .then(guard)
    .then((ok) => {
      if (ok) forceClose(id);
    })
    .catch(() => forceClose(id));
}
