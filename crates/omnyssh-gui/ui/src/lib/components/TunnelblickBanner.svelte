<script lang="ts">
  // Launch-time check (macOS): Tunnelblick is what brings up the OpenVPN
  // connections some servers need. When it is missing, offer to install it — the
  // app downloads the official notarized release, verifies its checksum and puts
  // it into Applications. "Don't ask again" silences the banner for good; the host
  // form still offers the install when a VPN is picked. Semantic tokens only.
  import { onMount } from 'svelte';
  import { Icon } from '$lib/theme';
  import { t } from '$lib/i18n';
  import { vpn, vpnInstallState, vpnBannerDismissed, loadVpnStatus, installTunnelblick } from '$lib/stores/vpn';

  let hiddenThisRun = $state(false);

  onMount(() => {
    void vpnBannerDismissed.hydrate();
    void loadVpnStatus();
  });

  const installing = $derived(!['idle', 'failed', 'done'].includes($vpnInstallState.stage));
  const show = $derived(
    !!$vpn &&
      $vpn.supported &&
      !hiddenThisRun &&
      (($vpnInstallState.stage !== 'idle') || (!$vpn.installed && !$vpnBannerDismissed))
  );

  const action =
    'rounded-full px-3 py-1.5 text-xs font-medium transition disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
</script>

{#if show}
  <div class="pointer-events-none fixed inset-x-0 bottom-14 z-40 flex justify-center px-4">
    <div
      class="pointer-events-auto flex w-full max-w-xl items-center gap-3 rounded-2xl border border-default bg-surface-raised px-4 py-3 shadow-soft"
      role="status"
      aria-label="Tunnelblick"
    >
      <span class="shrink-0 text-muted"><Icon name="lock" size={18} /></span>
      <div class="min-w-0 flex-1">
        {#if $vpnInstallState.stage === 'idle'}
          <p class="text-sm font-medium">{$t('vpn.banner.title')}</p>
          <p class="text-xs text-muted">{$t('vpn.banner.body', { version: $vpn?.installsVersion ?? '' })}</p>
        {:else if $vpnInstallState.stage === 'downloading'}
          <p class="text-sm">{$t('vpn.stage.downloading', { percent: $vpnInstallState.percent })}</p>
          <div class="mt-1.5 h-1 overflow-hidden rounded-full bg-surface-inset">
            <div class="h-full rounded-full bg-accent transition-[width]" style="width: {$vpnInstallState.percent}%"></div>
          </div>
        {:else if $vpnInstallState.stage === 'verifying'}
          <p class="text-sm">{$t('vpn.stage.verifying')}</p>
        {:else if $vpnInstallState.stage === 'installing'}
          <p class="text-sm">{$t('vpn.stage.installing')}</p>
        {:else if $vpnInstallState.stage === 'done'}
          <p class="text-sm text-status-ok">{$t('vpn.stage.done')}</p>
        {:else if $vpnInstallState.stage === 'failed'}
          <p class="text-sm text-status-crit">{$t('vpn.stage.failed', { error: $vpnInstallState.error })}</p>
        {/if}
      </div>
      <div class="flex shrink-0 items-center gap-1">
        {#if $vpnInstallState.stage === 'idle' || $vpnInstallState.stage === 'failed'}
          <button type="button" class="{action} bg-accent text-accent-fg hover:opacity-90" onclick={() => void installTunnelblick()}>
            {$t('vpn.banner.install')}
          </button>
          <button type="button" class="{action} text-muted hover:bg-surface-inset hover:text-fg" onclick={() => (hiddenThisRun = true)}>
            {$t('vpn.banner.later')}
          </button>
          {#if $vpnInstallState.stage === 'idle'}
            <button
              type="button"
              class="{action} text-faint hover:bg-surface-inset hover:text-fg"
              onclick={() => {
                vpnBannerDismissed.set(true);
                hiddenThisRun = true;
              }}
            >
              {$t('vpn.banner.never')}
            </button>
          {/if}
        {:else if !installing}
          <button type="button" class="{action} text-muted hover:bg-surface-inset hover:text-fg" onclick={() => (hiddenThisRun = true)}>
            {$t('common.close')}
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}
