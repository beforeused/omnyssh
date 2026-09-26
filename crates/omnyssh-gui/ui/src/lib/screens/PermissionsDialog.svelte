<script lang="ts">
  // chmod for the selection on the server: read / write / execute for owner, group
  // and others as checkboxes, the same mode as a number (either edits the other),
  // and — for folders — whether to apply it to everything inside. Semantic tokens only.
  import Modal from '$lib/components/Modal.svelte';
  import { Button } from '$lib/theme';
  import { t, type MessageKey } from '$lib/i18n';
  import { permString } from '$lib/stores/sftp';

  let {
    names,
    initial,
    hasFolders,
    onApply,
    onCancel
  }: {
    /** What is being changed, for the header. */
    names: string[];
    /** The current mode of the (first) entry. */
    initial: number;
    hasFolders: boolean;
    onApply: (mode: number, recursive: boolean) => void;
    onCancel: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let mode = $state(initial & 0o7777);
  let recursive = $state(false);
  // svelte-ignore state_referenced_locally
  let octal = $state((initial & 0o7777).toString(8).padStart(3, '0'));

  const WHO: Array<{ key: MessageKey; shift: number }> = [
    { key: 'fm.owner', shift: 6 },
    { key: 'fm.group', shift: 3 },
    { key: 'fm.others', shift: 0 }
  ];
  const WHAT: Array<{ key: MessageKey; bit: number }> = [
    { key: 'fm.read', bit: 4 },
    { key: 'fm.write', bit: 2 },
    { key: 'fm.exec', bit: 1 }
  ];

  function toggle(mask: number): void {
    mode ^= mask;
    octal = mode.toString(8).padStart(3, '0');
  }

  function typed(value: string): void {
    octal = value;
    if (/^[0-7]{3,4}$/.test(value)) mode = parseInt(value, 8);
  }

  const valid = $derived(/^[0-7]{3,4}$/.test(octal));
</script>

<Modal label={$t('fm.permissionsTitle')} onClose={onCancel}>
  <form
    class="flex min-h-0 flex-col"
    onsubmit={(e) => {
      e.preventDefault();
      if (valid) onApply(mode, recursive);
    }}
  >
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="text-sm font-semibold">{$t('fm.permissionsTitle')}</h2>
      <p class="mt-1 truncate font-mono text-xs text-muted" title={names.join(', ')}>
        {names.length === 1 ? names[0] : names.join(', ')}
      </p>
    </header>
    <div class="space-y-4 px-5 py-4">
      <table class="w-full text-sm">
        <thead>
          <tr class="text-xs text-muted">
            <th class="pb-2 text-left font-medium"></th>
            {#each WHAT as w (w.key)}
              <th class="pb-2 font-medium">{$t(w.key)}</th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each WHO as who (who.key)}
            <tr>
              <td class="py-1.5 text-muted">{$t(who.key)}</td>
              {#each WHAT as w (w.key)}
                {@const mask = w.bit << who.shift}
                <td class="py-1.5 text-center">
                  <input
                    type="checkbox"
                    class="accent-current"
                    aria-label="{$t(who.key)} — {$t(w.key)}"
                    checked={(mode & mask) !== 0}
                    onchange={() => toggle(mask)}
                  />
                </td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="flex items-center justify-between gap-3">
        <label class="flex items-center gap-2 text-xs text-muted">
          {$t('fm.octal')}
          <input
            value={octal}
            oninput={(e) => typed((e.currentTarget as HTMLInputElement).value)}
            maxlength="4"
            class="w-16 rounded-lg bg-surface-inset px-2 py-1 font-mono text-sm text-fg outline-none focus-visible:ring-2 focus-visible:ring-focus
              {valid ? '' : 'ring-2 ring-focus'}"
          />
        </label>
        <span class="font-mono text-xs text-faint">{permString(mode, false).slice(1)}</span>
      </div>
      {#if hasFolders}
        <label class="flex cursor-pointer items-center gap-2 text-xs text-muted">
          <input type="checkbox" class="accent-current" bind:checked={recursive} />
          {$t('fm.recursive')}
        </label>
      {/if}
    </div>
    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <Button variant="ghost" onclick={onCancel}>{$t('common.cancel')}</Button>
      <Button variant="primary" type="submit" disabled={!valid}>{$t('fm.apply')}</Button>
    </footer>
  </form>
</Modal>
