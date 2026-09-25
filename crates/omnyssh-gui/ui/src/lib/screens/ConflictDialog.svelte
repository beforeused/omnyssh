<script lang="ts">
  // "A file with this name already exists" — asked once per conflicting file, with
  // "apply to all" to answer the rest of the batch in one go. Escape / the scrim
  // cancels the whole batch. Semantic tokens only (§5.1).
  import { t } from '$lib/i18n';
  import Modal from '$lib/components/Modal.svelte';
  import type { ConflictActionDto, TransferConflictDto } from '$lib/bindings';
  import { formatBytes } from '$lib/stores/sftp';

  let {
    conflict,
    remaining,
    onAnswer,
    onCancel
  }: {
    conflict: TransferConflictDto;
    /** Conflicts left in this batch, this one included. */
    remaining: number;
    onAnswer: (action: ConflictActionDto, applyToAll: boolean) => void;
    onCancel: () => void;
  } = $props();

  let applyToAll = $state(false);

  const where = $derived.by(() => {
    const d = conflict.destination;
    const cut = Math.max(d.lastIndexOf('/'), d.lastIndexOf('\\'));
    if (cut < 0) return d;
    return cut === 0 ? d.slice(0, 1) : d.slice(0, cut);
  });
  const leaf = $derived(conflict.name.split('/').pop() ?? conflict.name);

  const btn =
    'rounded-full px-4 py-2 text-sm transition focus-visible:outline-none focus-visible:ring-2 ' +
    'focus-visible:ring-focus';
</script>

<Modal label={$t('conflict.title')} onClose={onCancel}>
  <header class="border-b border-default px-5 py-3.5">
    <h2 class="text-sm font-semibold">{$t('conflict.heading', { name: leaf })}</h2>
    <p class="mt-1 truncate text-xs text-muted" title={where}>{$t('conflict.in', { dir: where })}</p>
  </header>

  <div class="space-y-3 px-5 py-4 text-sm">
    <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
      <dt class="text-muted">{$t('conflict.existing')}</dt>
      <dd class="tabular-nums">
        {conflict.existingIsDir ? $t('conflict.aFolder') : formatBytes(conflict.existingSize)}
      </dd>
      <dt class="text-muted">{$t('conflict.incoming')}</dt>
      <dd class="tabular-nums">{formatBytes(conflict.sourceSize)}</dd>
    </dl>
    {#if conflict.existingIsDir}
      <p class="text-xs text-status-warn">{$t('conflict.folderNoReplace')}</p>
    {/if}
    {#if remaining > 1}
      <label class="flex cursor-pointer select-none items-center gap-2 text-xs text-muted">
        <input type="checkbox" bind:checked={applyToAll} class="accent-current" />
        {$t('conflict.applyAll', { count: remaining })}
      </label>
    {/if}
  </div>

  <footer class="flex flex-wrap justify-end gap-2 border-t border-default px-5 py-3">
    <button
      type="button"
      class="{btn} mr-auto text-muted hover:bg-surface-inset hover:text-fg"
      onclick={onCancel}
    >
      {$t('common.cancel')}
    </button>
    <button
      type="button"
      class="{btn} text-muted hover:bg-surface-inset hover:text-fg"
      onclick={() => onAnswer('skip', applyToAll)}
    >
      {$t('conflict.skip')}
    </button>
    <button
      type="button"
      class="{btn} border border-strong text-fg hover:bg-surface-inset"
      title={$t('conflict.keepBothTitle', { name: conflict.altName })}
      onclick={() => onAnswer('keepBoth', applyToAll)}
    >
      {$t('conflict.keepBoth')}
    </button>
    <button
      type="button"
      class="{btn} bg-accent font-medium text-accent-fg hover:opacity-90 disabled:opacity-40"
      disabled={conflict.existingIsDir}
      onclick={() => onAnswer('replace', applyToAll)}
    >
      {$t('conflict.replace')}
    </button>
  </footer>
</Modal>
