<script lang="ts">
  import { tick } from 'svelte';
  import Modal from './Modal.svelte';
  import { Button, Icon } from '$lib/theme';
  import { t } from '$lib/i18n';
  import { keyFileName } from '$lib/stores/keys';
  import { keyPassphrasePrompt } from '$lib/stores/keyPassphrase';
  import { unlockSshKey } from '$lib/ipc/commands';

  let passphrase = $state('');
  let error = $state<string | null>(null);
  let unlocking = $state(false);
  let input = $state<HTMLInputElement>();
  let shownPath = '';

  $effect(() => {
    const path = $keyPassphrasePrompt?.keyPath ?? '';
    if (path && path !== shownPath) {
      shownPath = path;
      passphrase = '';
      error = null;
      unlocking = false;
      void tick().then(() => input?.focus());
    }
  });

  function close(): void {
    if (!unlocking) keyPassphrasePrompt.answer(false);
  }

  async function submit(): Promise<void> {
    const request = $keyPassphrasePrompt;
    if (!request || !passphrase || unlocking) return;
    unlocking = true;
    error = null;
    try {
      await unlockSshKey(request.keyPath, passphrase);
      passphrase = '';
      keyPassphrasePrompt.answer(true);
    } catch {
      error = $t('keypass.invalid');
      passphrase = '';
      await tick();
      input?.focus();
    } finally {
      unlocking = false;
    }
  }
</script>

{#if $keyPassphrasePrompt}
  {@const request = $keyPassphrasePrompt}
  <Modal label={$t('keypass.title', { host: request.hostName })} onClose={close}>
    <form
      onsubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <header class="border-b border-default px-5 py-3.5">
        <div class="flex items-center gap-2.5">
          <Icon name="key" size={16} />
          <h2 class="min-w-0 truncate text-sm font-semibold">
            {$t('keypass.title', { host: request.hostName })}
          </h2>
        </div>
        <p class="mt-1 text-xs text-muted">
          {$t('keypass.body', { key: keyFileName(request.keyPath) })}
        </p>
      </header>

      <div class="space-y-2 px-5 py-4">
        <label class="block space-y-1.5">
          <span class="text-xs font-medium text-muted">{$t('keypass.label')}</span>
          <input
            bind:this={input}
            bind:value={passphrase}
            type="password"
            autocomplete="off"
            spellcheck="false"
            class="w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none focus-visible:ring-2 focus-visible:ring-focus"
          />
        </label>
        <p class="text-[11px] text-faint">{$t('keypass.memoryOnly')}</p>
        {#if error}<p class="text-xs text-status-crit">{error}</p>{/if}
      </div>

      <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
        <Button variant="ghost" onclick={close} disabled={unlocking}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="submit" disabled={!passphrase || unlocking}>
          {$t('keypass.connect')}
        </Button>
      </footer>
    </form>
  </Modal>
{/if}
