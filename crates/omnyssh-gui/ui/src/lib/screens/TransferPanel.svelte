<script lang="ts">
  // The transfer queue of one SFTP tab: a slim summary strip (count, overall bar,
  // speed, time left) that expands into the per-file list. It sits under the panes
  // instead of over them, so browsing carries on while files move. Semantic tokens
  // only (§5.1).
  import { t } from '$lib/i18n';
  import { Icon } from '$lib/theme';
  import {
    summarize,
    formatDuration,
    isActive,
    type TransferItem,
    type TransferQueue
  } from '$lib/stores/transfers';
  import { formatBytes } from '$lib/stores/sftp';

  let {
    queue,
    preparing,
    onCancel,
    onRetry,
    onDismiss,
    onClear
  }: {
    queue: TransferQueue;
    /** Batches still being walked/checked for conflicts. */
    preparing: number;
    onCancel: (ids: number[]) => void;
    onRetry: (ids: number[]) => void;
    onDismiss: (id: number) => void;
    onClear: () => void;
  } = $props();

  let expanded = $state(true);

  const summary = $derived(summarize(queue));
  const percent = $derived(
    summary.total > 0 ? Math.min(100, (summary.done / summary.total) * 100) : summary.active ? 0 : 100
  );
  const activeIds = $derived(queue.items.filter((i) => isActive(i.state)).map((i) => i.id));
  const failedIds = $derived(queue.items.filter((i) => i.state === 'failed').map((i) => i.id));
  // Running first, then waiting, then settled — newest settled on top.
  const ordered = $derived.by(() => {
    const rank = (i: TransferItem) =>
      i.state === 'running' ? 0 : i.state === 'queued' ? 1 : i.state === 'failed' ? 2 : 3;
    return [...queue.items].sort((a, b) => rank(a) - rank(b) || (rank(a) >= 2 ? b.id - a.id : a.id - b.id));
  });

  function itemPercent(i: TransferItem): number {
    if (i.state === 'done') return 100;
    return i.total > 0 ? Math.min(100, (i.done / i.total) * 100) : 0;
  }

  const headline = $derived(
    summary.active > 0
      ? $t('transfers.inProgress', { count: summary.active })
      : preparing > 0
        ? $t('transfers.preparing')
        : summary.failed > 0
          ? $t('transfers.failedN', { count: summary.failed })
          : $t('transfers.allDone')
  );

  const iconBtn =
    'grid h-6 w-6 shrink-0 place-items-center rounded-full text-faint transition hover:bg-surface-inset ' +
    'hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const textBtn =
    'rounded-full px-2.5 py-1 text-xs text-muted transition hover:bg-surface-inset hover:text-fg ' +
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
</script>

<section class="shrink-0 border-t border-default bg-surface" aria-label={$t('transfers.region')}>
  <div class="flex items-center gap-3 px-4 py-2">
    <button
      type="button"
      class={iconBtn}
      aria-label={expanded ? $t('transfers.collapse') : $t('transfers.expand')}
      aria-expanded={expanded}
      onclick={() => (expanded = !expanded)}
    >
      <Icon name={expanded ? 'chevron-down' : 'chevron-up'} size={14} />
    </button>
    <div class="min-w-0 flex-1">
      <div class="flex items-center justify-between gap-3 text-xs">
        <span class="truncate font-medium text-fg">{headline}</span>
        <span class="shrink-0 tabular-nums text-muted">
          {#if summary.active > 0}
            {formatBytes(summary.done)} / {formatBytes(summary.total)}
            {#if summary.speed > 0}· {formatBytes(summary.speed)}/s{/if}
            {#if summary.eta !== undefined}· {$t('transfers.left', { time: formatDuration(summary.eta) })}{/if}
          {:else if summary.finished > 0}
            {$t('transfers.doneN', { count: summary.finished })}
          {/if}
        </span>
      </div>
      <div class="mt-1.5 h-1 overflow-hidden rounded-full bg-surface-inset">
        <div
          class="h-full rounded-full transition-[width] duration-300 {summary.failed && !summary.active
            ? 'bg-status-crit'
            : 'bg-accent'}"
          style="width: {percent}%"
        ></div>
      </div>
    </div>
    <div class="flex shrink-0 items-center gap-1">
      {#if failedIds.length}
        <button type="button" class={textBtn} onclick={() => onRetry(failedIds)}>{$t('transfers.retryFailed')}</button>
      {/if}
      {#if activeIds.length}
        <button type="button" class={textBtn} onclick={() => onCancel(activeIds)}>{$t('transfers.cancelAll')}</button>
      {:else}
        <button type="button" class={textBtn} onclick={onClear}>{$t('transfers.clear')}</button>
      {/if}
    </div>
  </div>

  {#if expanded && ordered.length}
    <ul class="max-h-48 overflow-y-auto border-t border-default px-2 py-1" aria-label={$t('transfers.list')}>
      {#each ordered as item (item.id)}
        <li class="flex items-center gap-3 rounded px-2 py-1.5 text-xs">
          <span class="shrink-0 text-faint" title={item.direction === 'upload' ? $t('transfers.upload') : $t('transfers.download')}>
            <Icon name={item.direction === 'upload' ? 'upload' : 'download'} size={13} />
          </span>
          <div class="min-w-0 flex-1">
            <div class="flex items-center justify-between gap-3">
              <span
                class="min-w-0 truncate font-mono {item.state === 'done' || item.state === 'cancelled'
                  ? 'text-muted'
                  : 'text-fg'}"
                title={item.direction === 'upload' ? item.remote : item.local}
              >
                {item.name}
              </span>
              <span
                class="shrink-0 tabular-nums {item.state === 'failed' ? 'text-status-crit' : 'text-faint'}"
              >
                {#if item.state === 'running'}
                  {formatBytes(item.done)} / {formatBytes(item.total)}{#if item.speed > 0}
                    · {formatBytes(item.speed)}/s{/if}
                {:else if item.state === 'queued'}
                  {$t('transfers.waiting')} · {formatBytes(item.total)}
                {:else if item.state === 'done'}
                  {formatBytes(item.total)}
                {:else if item.state === 'cancelled'}
                  {$t('transfers.cancelled')}
                {:else}
                  {$t('transfers.failed')}
                {/if}
              </span>
            </div>
            {#if item.state === 'failed' && item.error}
              <p class="mt-0.5 truncate text-status-crit" title={item.error}>{item.error}</p>
            {:else if isActive(item.state)}
              <div class="mt-1 h-0.5 overflow-hidden rounded-full bg-surface-inset">
                <div
                  class="h-full rounded-full bg-accent transition-[width] duration-300"
                  style="width: {itemPercent(item)}%"
                ></div>
              </div>
            {/if}
          </div>
          {#if isActive(item.state)}
            <button
              type="button"
              class={iconBtn}
              aria-label={$t('transfers.cancelOf', { name: item.name })}
              title={$t('transfers.cancel')}
              onclick={() => onCancel([item.id])}
            >
              <Icon name="close" size={12} />
            </button>
          {:else if item.state === 'failed' || item.state === 'cancelled'}
            <button
              type="button"
              class={iconBtn}
              aria-label={$t('transfers.retryOf', { name: item.name })}
              title={$t('transfers.retry')}
              onclick={() => onRetry([item.id])}
            >
              <Icon name="refresh" size={12} />
            </button>
            <button
              type="button"
              class={iconBtn}
              aria-label={$t('transfers.dismissOf', { name: item.name })}
              title={$t('transfers.dismiss')}
              onclick={() => onDismiss(item.id)}
            >
              <Icon name="close" size={12} />
            </button>
          {:else}
            <span class="grid h-6 w-6 shrink-0 place-items-center text-status-ok" aria-label={$t('transfers.done')}>
              <Icon name="check" size={13} />
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>
