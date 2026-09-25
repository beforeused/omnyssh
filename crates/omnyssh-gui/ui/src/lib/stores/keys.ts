import { writable } from 'svelte/store';
import type { SshKeyDto } from '$lib/bindings';
import { listSshKeys, getDefaultKey, setDefaultKey as saveDefaultKey } from '$lib/ipc/commands';

// The SSH keys the pickers offer (found in ~/.ssh) and the app-wide default key.
// Loaded on demand — a picker opening calls `refreshKeys` — so a key created in a
// terminal a minute ago shows up without restarting the app.

export const sshKeys = writable<SshKeyDto[]>([]);
export const defaultKey = writable<string | null>(null);

export async function refreshKeys(): Promise<void> {
  try {
    const keys = await listSshKeys();
    sshKeys.set(Array.isArray(keys) ? keys : []);
  } catch {
    // Not under Tauri, or ~/.ssh unreadable: keep what we have.
  }
}

export async function loadDefaultKey(): Promise<void> {
  try {
    defaultKey.set((await getDefaultKey()) ?? null);
  } catch {
    // Keep the current value.
  }
}

export async function setDefaultKey(path: string | null): Promise<void> {
  await saveDefaultKey(path);
  defaultKey.set(path);
}

/** "id_ed25519" from a path, for labels. */
export function keyFileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** Open the OS file picker in ~/.ssh and return the chosen file, if any. */
export async function browseForKey(title: string): Promise<string | null> {
  try {
    const [{ open }, { homeDir, join }] = await Promise.all([
      import('@tauri-apps/plugin-dialog'),
      import('@tauri-apps/api/path')
    ]);
    let defaultPath: string | undefined;
    try {
      defaultPath = await join(await homeDir(), '.ssh');
    } catch {
      defaultPath = undefined;
    }
    const picked = await open({ title, multiple: false, directory: false, defaultPath });
    return typeof picked === 'string' && picked ? picked : null;
  } catch {
    return null;
  }
}
