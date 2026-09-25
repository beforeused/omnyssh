<script lang="ts">
  // Per-host snippet results (tech-gui.md §2.2), driven by the `snippetRun` store:
  // each target host shows pending → ok/failed with its output. Colour lives only in
  // the status dot, per the brandbook. Rendered whenever a run is active.
  import { t } from '$lib/i18n';
  import { StatusDot, Button, type Status } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import { snippetRun, clearRun, type SnippetResultEntry } from '$lib/stores/snippets';

  function dot(entry: SnippetResultEntry): Status {
    if (entry.pending) return 'unknown';
    return entry.ok ? 'ok' : 'crit';
  }

  function state(entry: SnippetResultEntry): string {
    if (entry.pending) return $t('snip.running');
    return entry.ok ? $t('snip.ok') : $t('snip.failed');
  }
</script>

{#if $snippetRun}
  {@const run = $snippetRun}
  <Modal label={$t('snip.results')} onClose={clearRun}>
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="truncate text-sm font-semibold">{$t('snip.resultsHeading', { name: run.snippetName })}</h2>
    </header>

    <div class="min-h-0 flex-1 space-y-3 overflow-y-auto px-5 py-4">
      {#each run.entries as entry (entry.hostName)}
        <div class="rounded-lg border border-default">
          <div class="flex items-center gap-2.5 border-b border-default px-3 py-2">
            <StatusDot status={dot(entry)} label="{entry.hostName} {state(entry)}" />
            <span class="min-w-0 flex-1 truncate text-sm font-medium">{entry.hostName}</span>
            <span class="shrink-0 text-[11px] uppercase tracking-wider text-faint">{state(entry)}</span>
          </div>
          {#if entry.pending}
            <p class="px-3 py-2 text-xs text-faint">{$t('snip.runningLong')}</p>
          {:else if entry.output.trim()}
            <pre
              class="max-h-52 select-text overflow-auto whitespace-pre-wrap break-words px-3 py-2 font-mono text-xs {entry.ok
                ? 'text-muted'
                : 'text-status-crit'}">{entry.output}</pre>
          {:else}
            <p class="px-3 py-2 text-xs text-faint">{$t('snip.noOutput')}</p>
          {/if}
        </div>
      {/each}
    </div>

    <footer class="flex justify-end border-t border-default px-5 py-3">
      <Button variant="secondary" onclick={clearRun}>{$t('common.close')}</Button>
    </footer>
  </Modal>
{/if}
