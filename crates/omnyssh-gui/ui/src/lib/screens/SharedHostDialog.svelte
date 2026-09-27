<script lang="ts">
  import { get } from 'svelte/store';
  import Modal from '$lib/components/Modal.svelte';
  import KeyPicker from '$lib/components/KeyPicker.svelte';
  import { Button, Icon } from '$lib/theme';
  import { t } from '$lib/i18n';
  import { hosts } from '$lib/stores/hosts';
  import { defaultKey, keyFileName, loadDefaultKey, refreshKeys } from '$lib/stores/keys';
  import { reloadHosts, saveHost } from '$lib/ipc/commands';
  import {
    availableSharedHostName,
    sharedHostInput,
    type SharedHost
  } from '$lib/share/hostShare';

  let { shared, onClose }: { shared: SharedHost; onClose: () => void } = $props();
  // The dialog is remounted for each pasted payload, so this intentionally seeds
  // one editable value instead of following a later prop change.
  // svelte-ignore state_referenced_locally
  let name = $state(availableSharedHostName(shared.name, get(hosts).map((host) => host.name)));
  let identityFile = $state('');
  let saving = $state(false);
  let error = $state<string | null>(null);

  void Promise.all([refreshKeys(), loadDefaultKey()]);

  const emptyKeyLabel = $derived(
    $defaultKey ? $t('keys.defaultIs', { name: keyFileName($defaultKey) }) : $t('keys.none')
  );

  async function importHost(): Promise<void> {
    error = null;
    if (get(hosts).some((host) => host.name === name.trim())) {
      error = $t('dash.exists', { name: name.trim() });
      return;
    }
    saving = true;
    try {
      await saveHost(sharedHostInput(shared, name, identityFile));
      await reloadHosts();
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

<Modal label={$t('share.importTitle')} {onClose}>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void importHost();
    }}
    class="flex min-h-0 flex-col"
  >
    <header class="border-b border-default px-5 py-3.5">
      <div class="flex items-center gap-2.5">
        <Icon name="share" size={16} />
        <h2 class="text-sm font-semibold">{$t('share.importTitle')}</h2>
      </div>
      <p class="mt-1 text-xs text-muted">{$t('share.review')}</p>
    </header>

    <div class="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
      <div class="rounded-xl border border-default bg-surface-inset px-3.5 py-3">
        <p class="font-mono text-sm text-fg">{shared.user}@{shared.hostname}:{shared.port}</p>
        {#if shared.vpn}<p class="mt-1 text-xs text-muted">VPN: {shared.vpn}</p>{/if}
      </div>

      <p class="rounded-lg border border-default px-3 py-2 text-xs text-muted">
        {$t('share.safeNote')}
      </p>

      <label class={label}>
        <span>{$t('host.name')}</span>
        <input bind:value={name} class={field} maxlength="128" />
      </label>

      <div class={label}>
        <label for="shared-host-key">{$t('share.localKey')}</label>
        <KeyPicker bind:value={identityFile} emptyLabel={emptyKeyLabel} id="shared-host-key" />
        <p class="text-[11px] font-normal text-faint">{$t('share.localKeyHint')}</p>
      </div>

      {#if error}<p class="text-xs text-status-crit">{error}</p>{/if}
    </div>

    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <Button variant="ghost" onclick={onClose}>{$t('common.cancel')}</Button>
      <Button variant="primary" type="submit" disabled={saving || !name.trim()}>
        {$t('share.add')}
      </Button>
    </footer>
  </form>
</Modal>
