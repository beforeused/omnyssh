<script lang="ts">
  import { get } from 'svelte/store';
  import Modal from '$lib/components/Modal.svelte';
  import { Button, Icon } from '$lib/theme';
  import { t, type MessageKey } from '$lib/i18n';
  import { createSshKey } from '$lib/ipc/commands';
  import { refreshKeys, sshKeys } from '$lib/stores/keys';

  let { onClose }: { onClose: () => void } = $props();

  function availableName(): string {
    const names = new Set(get(sshKeys).map((key) => key.name));
    const base = 'omnyssh_ed25519';
    if (!names.has(base)) return base;
    let suffix = 2;
    while (names.has(`${base}_${suffix}`)) suffix += 1;
    return `${base}_${suffix}`;
  }

  let name = $state(availableName());
  let comment = $state('');
  let passphrase = $state('');
  let confirmation = $state('');
  let saving = $state(false);
  let error = $state<string | null>(null);

  const nameError = $derived.by((): MessageKey | null => {
    const value = name.trim();
    if (!value) return 'keysetup.name.empty';
    if (value.length > 64) return 'keysetup.name.long';
    if (!/^[A-Za-z0-9._-]+$/.test(value)) return 'keysetup.name.chars';
    if (value.startsWith('.') || value.startsWith('-')) return 'keysetup.name.start';
    if (value.endsWith('.pub')) return 'keysetup.name.pub';
    if (['config', 'authorized_keys', 'environment'].includes(value) || value.startsWith('known_hosts')) {
      return 'keysetup.name.reserved';
    }
    if (get(sshKeys).some((key) => key.name === value)) return 'keysetup.name.exists';
    return null;
  });
  const passphraseError = $derived(
    passphrase !== confirmation ? 'keys.create.passphraseMismatch' : null
  );
  const canCreate = $derived(!saving && nameError === null && passphraseError === null);

  async function create(): Promise<void> {
    if (!canCreate) return;
    saving = true;
    error = null;
    try {
      await createSshKey(name.trim(), comment.trim(), passphrase);
      passphrase = '';
      confirmation = '';
      await refreshKeys();
      onClose();
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
    } finally {
      saving = false;
    }
  }

  const label = 'block space-y-1.5 text-xs font-medium text-muted';
  const field =
    'w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none ' +
    'focus-visible:ring-2 focus-visible:ring-focus';
</script>

<Modal label={$t('keys.create.title')} {onClose}>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void create();
    }}
    class="flex min-h-0 flex-col"
  >
    <header class="border-b border-default px-5 py-3.5">
      <div class="flex items-center gap-2.5">
        <Icon name="key" size={16} />
        <h2 class="text-sm font-semibold">{$t('keys.create.title')}</h2>
      </div>
      <p class="mt-1 text-xs text-muted">{$t('keys.create.body')}</p>
    </header>

    <div class="min-h-0 flex-1 space-y-3.5 overflow-y-auto px-5 py-4">
      <label class={label}>
        <span>{$t('keys.create.name')}</span>
        <span class="flex items-center rounded-lg bg-surface-inset font-mono text-sm focus-within:ring-2 focus-within:ring-focus">
          <span class="shrink-0 pl-3 text-faint">~/.ssh/</span>
          <input bind:value={name} class="min-w-0 flex-1 bg-transparent py-2 pr-3 text-fg outline-none" />
        </span>
      </label>
      {#if nameError}<p class="text-xs text-status-crit">{$t(nameError, { name: name.trim() })}</p>{/if}

      <label class={label}>
        <span>{$t('keys.create.comment')}</span>
        <input bind:value={comment} class={field} placeholder="name@computer" autocomplete="off" />
      </label>

      <label class={label}>
        <span>{$t('keys.create.passphrase')}</span>
        <input bind:value={passphrase} type="password" class={field} autocomplete="new-password" />
      </label>
      <label class={label}>
        <span>{$t('keys.create.confirm')}</span>
        <input bind:value={confirmation} type="password" class={field} autocomplete="new-password" />
      </label>
      {#if passphraseError}<p class="text-xs text-status-crit">{$t(passphraseError)}</p>{/if}
      <p class="text-[11px] text-faint">{$t('keys.create.security')}</p>
      {#if error}<p class="text-xs text-status-crit">{error}</p>{/if}
    </div>

    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <Button variant="ghost" onclick={onClose}>{$t('common.cancel')}</Button>
      <Button variant="primary" type="submit" disabled={!canCreate}>{$t('keys.create.action')}</Button>
    </footer>
  </form>
</Modal>
