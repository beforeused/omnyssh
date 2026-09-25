<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { startEventBridge } from '$lib/ipc/subscribe';
  import { reloadHosts, refreshMetrics } from '$lib/ipc/commands';
  import { theme } from '$lib/stores/theme';
  import { sidebarCollapsed } from '$lib/stores/ui';
  import { streamerMode } from '$lib/stores/streamer';
  import {
    refreshInterval,
    driveMetricsRefresh,
    editor,
    transferStreams
  } from '$lib/stores/settings';
  import { setTransferStreams } from '$lib/ipc/commands';
  import { locale } from '$lib/i18n';
  import { lastError } from '$lib/stores/notifications';

  let { children } = $props();

  onMount(() => {
    let stop: (() => void) | undefined;
    let disposed = false;
    // Reconcile the persisted prefs with their canonical tauri-plugin-store values;
    // the synchronous localStorage mirrors already seeded the first paint (§5.1, §2).
    void theme.hydrate();
    void sidebarCollapsed.hydrate();
    void streamerMode.hydrate();
    void refreshInterval.hydrate();
    void editor.hydrate();
    void locale.hydrate();
    // Screen readers and hyphenation follow the UI language.
    const stopLang = locale.subscribe((l) => document.documentElement.setAttribute('lang', l));
    void transferStreams.hydrate();
    // Keep the backend's per-host connection count in step with the setting.
    const stopStreams = transferStreams.subscribe((n) => {
      void setTransferStreams(n).catch(() => {});
    });
    // Force a metric refresh on the user's interval; re-arms when the interval changes.
    const stopRefresh = driveMetricsRefresh(() => {
      void refreshMetrics().catch(() => {});
    });
    // No-op outside Tauri (e.g. a plain `vite preview`); the shell still mounts.
    // Dispose even if the layout unmounts before the subscription resolves. Start
    // the pollers only once listeners are attached, so no status event is missed.
    startEventBridge()
      .then((off) => {
        if (disposed) return off();
        stop = off;
        reloadHosts().catch((err) => lastError.set(err instanceof Error ? err.message : String(err)));
      })
      .catch(() => {});
    return () => {
      disposed = true;
      stop?.();
      stopRefresh();
      stopStreams();
      stopLang();
    };
  });
</script>

{@render children()}
