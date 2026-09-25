<script lang="ts">
  // Bottom region (tech-gui.md §2): context + host summary, and where background
  // errors surface (§3.5). The summary counts total / online / alert / offline
  // (§4.1); colour lives only in the status dots, per the brandbook.
  import { t } from '$lib/i18n';
  import { lastError } from '$lib/stores/notifications';
  import { hostSummary } from '$lib/stores/hostSummary';
  import { StatusDot } from '$lib/theme';
</script>

<footer
  class="col-span-2 col-start-1 row-start-2 flex items-center justify-between gap-4 border-t border-default bg-surface px-5 py-2 text-xs text-muted"
>
  {#if $lastError}
    <span class="min-w-0 truncate text-status-crit">{$lastError}</span>
  {:else}
    <span class="min-w-0 truncate">{$t('status.ready')}</span>
  {/if}
  <div class="flex shrink-0 items-center gap-3">
    <span>{$t('status.hosts', { count: $hostSummary.total })}</span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="ok" label={$t('status.dotOnline')} />{$t('status.online', { count: $hostSummary.online })}
    </span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="warn" label={$t('status.dotAlert')} />{$t('status.alert', { count: $hostSummary.alert })}
    </span>
    <span class="flex items-center gap-1.5">
      <StatusDot status="off" label={$t('status.dotOffline')} />{$t('status.offline', { count: $hostSummary.offline })}
    </span>
  </div>
</footer>
