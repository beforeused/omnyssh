import { writable, get } from 'svelte/store';
import type { VpnInstallProgress, VpnStatusDto } from '$lib/bindings';
import { vpnStatus, vpnInstall, vpnConfigurations } from '$lib/ipc/commands';
import { createPref } from './settings';

// Tunnelblick (OpenVPN on macOS): whether it is there, installing it, and its
// configuration names for the host form. Install progress arrives as
// `vpn-install-progress` events (routed by ipc/router).

export const vpn = writable<VpnStatusDto | null>(null);

export type InstallState =
  | { stage: 'idle' }
  | { stage: 'downloading'; percent: number }
  | { stage: 'verifying' | 'installing' | 'done' }
  | { stage: 'failed'; error: string };

export const vpnInstallState = writable<InstallState>({ stage: 'idle' });
export const vpnConfigs = writable<string[]>([]);
/** Why the last configuration listing failed (shown in the host form), or null. */
export const vpnConfigsError = writable<string | null>(null);

/** "Don't ask again" for the launch-time install banner. */
export const vpnBannerDismissed = createPref<boolean>(
  'omnyssh-vpn-banner-dismissed',
  'vpnBannerDismissed',
  false,
  (raw) => raw === true
);

export async function loadVpnStatus(): Promise<VpnStatusDto | null> {
  try {
    const status = await vpnStatus();
    vpn.set(status ?? null);
    return status ?? null;
  } catch {
    return null;
  }
}

export async function loadVpnConfigs(): Promise<void> {
  try {
    const names = await vpnConfigurations();
    vpnConfigs.set(Array.isArray(names) ? names : []);
    vpnConfigsError.set(null);
  } catch (e) {
    vpnConfigs.set([]);
    vpnConfigsError.set(e instanceof Error ? e.message : String(e));
  }
}

export async function installTunnelblick(): Promise<void> {
  const current = get(vpnInstallState).stage;
  if (current !== 'idle' && current !== 'failed') return;
  vpnInstallState.set({ stage: 'downloading', percent: 0 });
  try {
    await vpnInstall();
    vpnInstallState.set({ stage: 'done' });
  } catch (e) {
    vpnInstallState.set({ stage: 'failed', error: e instanceof Error ? e.message : String(e) });
  }
  await loadVpnStatus();
}

/** Fold a `vpn-install-progress` event in. Pure over the payload. */
export function installStateFrom(p: VpnInstallProgress): InstallState {
  switch (p.stage) {
    case 'downloading':
      return { stage: 'downloading', percent: p.total > 0 ? Math.min(100, Math.round((p.done / p.total) * 100)) : 0 };
    case 'failed':
      return { stage: 'failed', error: p.error ?? '' };
    default:
      return { stage: p.stage };
  }
}
