<script lang="ts">
  // The server card's key button: choose which key to install on the host — one of
  // the keys in ~/.ssh (shown as a list, or any other file via the picker), or a new
  // one under a name of your choice — and how the server should accept logins
  // afterwards: password and key, or key only. Starting hands over to the progress
  // panel (KeySetupProgress). Semantic tokens only.
  import { onMount } from 'svelte';
  import Modal from '$lib/components/Modal.svelte';
  import { Button, Icon } from '$lib/theme';
  import { t, type MessageKey } from '$lib/i18n';
  import type { AuthModeDto, HostDto, SshKeyDto } from '$lib/bindings';
  import {
    hostAuth,
    inspectSshKey,
    installPublicKey,
    keyPassphraseRequired,
    startKeySetup,
    unlockSshKey
  } from '$lib/ipc/commands';
  import {
    sshKeys,
    defaultKey,
    loadDefaultKey,
    refreshKeys,
    browseForKey,
    keyFileName
  } from '$lib/stores/keys';
  import { beginKeySetup, dismissKeySetup } from '$lib/stores/keySetup';
  import { lastError } from '$lib/stores/notifications';
  import { prepareHostAuthentication } from '$lib/stores/navigation';

  let { host, onClose }: { host: HostDto; onClose: () => void } = $props();

  let currentKey = $state<string | null>(null);
  let tab = $state<'existing' | 'new' | 'public'>('existing');
  let keyPath = $state('');
  /** Keys picked by hand from outside ~/.ssh, listed alongside the found ones. */
  let extraKeys = $state<SshKeyDto[]>([]);
  let pickError = $state<string | null>(null);
  let passphrase = $state('');
  let needsPassphrase = $state(false);
  let passphraseError = $state<string | null>(null);
  let keyCheck = 0;
  // svelte-ignore state_referenced_locally
  let newName = $state(`omnyssh_${host.name.replace(/[^A-Za-z0-9._-]/g, '_').slice(0, 64) || 'unnamed_host'}_ed25519`);
  // svelte-ignore state_referenced_locally
  let mode = $state<AuthModeDto>(host.passwordAuthDisabled ? 'keyOnly' : 'keyAndPassword');
  let starting = $state(false);
  let publicKey = $state('');
  let publicError = $state<string | null>(null);
  let publicDone = $state(false);

  const keys = $derived([
    ...$sshKeys,
    ...extraKeys.filter((e) => !$sshKeys.some((k) => k.path === e.path))
  ]);

  /** Mirrors the core's `validate_key_file_name`, plus "already in ~/.ssh". */
  const nameError = $derived.by((): MessageKey | null => {
    const n = newName.trim();
    if (!n) return 'keysetup.name.empty';
    if (n.length > 64) return 'keysetup.name.long';
    if (!/^[A-Za-z0-9._-]+$/.test(n)) return 'keysetup.name.chars';
    if (n.startsWith('.') || n.startsWith('-')) return 'keysetup.name.start';
    if (n.endsWith('.pub')) return 'keysetup.name.pub';
    if (['config', 'authorized_keys', 'environment'].includes(n) || n.startsWith('known_hosts')) {
      return 'keysetup.name.reserved';
    }
    if ($sshKeys.some((k) => k.name === n)) return 'keysetup.name.exists';
    return null;
  });

  const canStart = $derived(
    !starting &&
      (tab === 'new'
        ? nameError === null
        : tab === 'existing'
          ? keyPath !== '' && (!needsPassphrase || passphrase !== '')
          : publicKey.trim() !== '')
  );

  async function chooseKey(path: string): Promise<void> {
    keyPath = path;
    passphrase = '';
    passphraseError = null;
    const encrypted = keys.find((key) => key.path === path)?.encrypted ?? false;
    needsPassphrase = encrypted;
    const check = ++keyCheck;
    if (!encrypted) return;
    try {
      const required = await keyPassphraseRequired(path);
      if (check === keyCheck) needsPassphrase = required;
    } catch {
      // The install command will report a disappeared/unreadable key. Keep the
      // passphrase field visible meanwhile rather than silently treating it as plain.
    }
  }

  onMount(async () => {
    await Promise.all([refreshKeys(), loadDefaultKey()]);
    try {
      currentKey = (await hostAuth(host.name)).identityFile ?? null;
    } catch {
      currentKey = null;
    }
    // Preselect: the host's own key, else the default key, else the first key found.
    const initial = currentKey ?? $defaultKey ?? $sshKeys[0]?.path ?? '';
    if (initial) {
      if (!$sshKeys.some((k) => k.path === initial)) {
        try {
          extraKeys = [await inspectSshKey(initial)];
        } catch {
          extraKeys = [{ path: initial, name: keyFileName(initial), encrypted: false }];
        }
      }
      await chooseKey(initial);
    } else {
      tab = 'new';
    }
  });

  async function pickFile(): Promise<void> {
    pickError = null;
    const path = await browseForKey($t('keys.pickTitle'));
    if (!path) return;
    try {
      const key = await inspectSshKey(path);
      if (!keys.some((k) => k.path === key.path)) extraKeys = [...extraKeys, key];
      await chooseKey(key.path);
    } catch {
      pickError = $t('keys.notAKey', { path });
    }
  }

  async function start(): Promise<void> {
    if (!canStart) return;
    starting = true;
    if (tab === 'public') {
      publicError = null;
      try {
        if (!(await prepareHostAuthentication(host.name))) return;
        await installPublicKey(host.name, publicKey.trim());
        publicKey = '';
        publicDone = true;
      } catch (e) {
        publicError = e instanceof Error ? e.message : String(e);
      } finally {
        starting = false;
      }
      return;
    }
    passphraseError = null;
    if (tab === 'existing' && needsPassphrase) {
      try {
        await unlockSshKey(keyPath, passphrase);
        passphrase = '';
        needsPassphrase = false;
      } catch {
        passphrase = '';
        passphraseError = $t('keypass.invalid');
        starting = false;
        return;
      }
    }
    // Read everything before closing: the owner drops this dialog (and its props)
    // the moment `onClose` runs.
    const hostName = host.name;
    const key =
      tab === 'new'
        ? { kind: 'generate' as const, name: newName.trim() }
        : { kind: 'existing' as const, path: keyPath };
    const chosenMode = mode;
    beginKeySetup(hostName);
    onClose();
    try {
      await startKeySetup(hostName, key, chosenMode);
    } catch (e) {
      dismissKeySetup();
      lastError.set(e instanceof Error ? e.message : String(e));
    }
  }

  const section = 'mb-2 text-xs font-semibold uppercase tracking-[0.14em] text-muted';
  const tabBtn =
    'flex-1 rounded-lg px-3 py-1.5 text-sm transition focus-visible:outline-none ' +
    'focus-visible:ring-2 focus-visible:ring-focus';
  const tabState = (on: boolean): string =>
    on ? 'bg-accent text-accent-fg' : 'text-muted hover:bg-surface hover:text-fg';
  const card =
    'flex w-full items-center gap-3 rounded-xl border px-3.5 py-2.5 text-left transition ' +
    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const cardState = (on: boolean): string =>
    on ? 'border-strong bg-surface-inset' : 'border-default hover:border-strong';
  const badge = 'rounded-full border border-default px-1.5 py-px text-[10px] text-faint';
  const radio =
    'flex cursor-pointer items-start gap-3 rounded-xl border px-3.5 py-3 text-left transition ' +
    'focus-within:ring-2 focus-within:ring-focus';
</script>

<Modal label={$t('keysetup.title', { host: host.name })} {onClose}>
  <form
    class="flex min-h-0 flex-col"
    onsubmit={(e) => {
      e.preventDefault();
      void start();
    }}
  >
    <header class="border-b border-default px-5 py-3.5">
      <div class="flex items-center gap-2.5">
        <Icon name="key" size={16} />
        <h2 class="min-w-0 truncate text-sm font-semibold">{$t('keysetup.title', { host: host.name })}</h2>
      </div>
      <p class="mt-1 truncate text-xs text-muted" title={currentKey ?? undefined}>
        {$t('keysetup.current')}:
        {#if currentKey}
          <span class="font-mono text-fg">{keyFileName(currentKey)}</span>
        {:else}
          {$t('keysetup.currentNone')}
        {/if}
      </p>
    </header>

    <div class="min-h-0 flex-1 space-y-5 overflow-y-auto px-5 py-4">
      <section>
        <h3 class={section}>{$t('keysetup.keySection')}</h3>
        <div class="mb-3 flex gap-1 rounded-xl bg-surface-inset p-1" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={tab === 'existing'}
            class="{tabBtn} {tabState(tab === 'existing')}"
            onclick={() => (tab = 'existing')}
          >
            {$t('keysetup.tabExisting')}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === 'new'}
            class="{tabBtn} {tabState(tab === 'new')}"
            onclick={() => (tab = 'new')}
          >
            {$t('keysetup.tabNew')}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === 'public'}
            class="{tabBtn} {tabState(tab === 'public')}"
            onclick={() => (tab = 'public')}
          >
            {$t('keysetup.tabPublic')}
          </button>
        </div>

        {#if tab === 'existing'}
          <p class="mb-2 text-xs text-faint">{$t('keysetup.existingHint')}</p>
          <div class="space-y-1.5" role="radiogroup" aria-label={$t('keysetup.tabExisting')}>
            {#if keys.length === 0}
              <p class="rounded-xl border border-dashed border-default px-3.5 py-3 text-xs text-muted">
                {$t('keysetup.noKeys')}
              </p>
            {/if}
            {#each keys as k (k.path)}
              {@const on = keyPath === k.path}
              <button
                type="button"
                role="radio"
                aria-checked={on}
                aria-label={k.name}
                class="{card} {cardState(on)}"
                title={k.path}
                onclick={() => void chooseKey(k.path)}
              >
                <span
                  class="grid h-8 w-8 shrink-0 place-items-center rounded-full border
                    {on ? 'border-accent bg-accent text-accent-fg' : 'border-default text-muted'}"
                >
                  <Icon name={on ? 'check' : 'key'} size={14} />
                </span>
                <span class="min-w-0 flex-1">
                  <span class="flex min-w-0 flex-wrap items-center gap-1.5">
                    <span class="truncate font-mono text-sm text-fg">{k.name}</span>
                    {#if k.kind}<span class={badge}>{k.kind}</span>{/if}
                    {#if k.encrypted}<span class={badge}>{$t('keys.encrypted')}</span>{/if}
                    {#if currentKey === k.path}<span class={badge}>{$t('keysetup.inUse')}</span>{/if}
                    {#if $defaultKey === k.path}<span class={badge}>{$t('keysetup.default')}</span>{/if}
                  </span>
                  <span class="block truncate text-xs text-faint">{k.comment ?? k.path}</span>
                </span>
              </button>
            {/each}
            <button
              type="button"
              class="{card} border-dashed border-default text-muted hover:border-strong hover:text-fg"
              onclick={() => void pickFile()}
            >
              <span class="grid h-8 w-8 shrink-0 place-items-center rounded-full border border-dashed border-default">
                <Icon name="folder" size={14} />
              </span>
              <span class="min-w-0 flex-1">
                <span class="block text-sm">{$t('keysetup.otherFile')}</span>
                <span class="block text-xs text-faint">{$t('keysetup.otherFileHint')}</span>
              </span>
            </button>
            {#if pickError}
              <p class="text-xs text-status-crit">{pickError}</p>
            {/if}
            {#if keys.find((k) => k.path === keyPath)?.encrypted}
              <p class="text-xs text-status-warn">{$t('keysetup.encryptedWarn')}</p>
              {#if needsPassphrase}
                <label class="block space-y-1.5 pt-1">
                  <span class="text-xs font-medium text-muted">{$t('keypass.label')}</span>
                  <input
                    bind:value={passphrase}
                    type="password"
                    autocomplete="off"
                    spellcheck="false"
                    class="w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none focus-visible:ring-2 focus-visible:ring-focus"
                  />
                </label>
                <p class="text-[11px] text-faint">{$t('keypass.memoryOnly')}</p>
              {/if}
              {#if passphraseError}
                <p class="text-xs text-status-crit">{passphraseError}</p>
              {/if}
            {/if}
          </div>
        {:else if tab === 'new'}
          <label class="block space-y-1.5">
            <span class="text-xs font-medium text-muted">{$t('keysetup.keyName')}</span>
            <span
              class="flex items-center rounded-lg bg-surface-inset font-mono text-sm focus-within:ring-2 focus-within:ring-focus"
            >
              <span class="shrink-0 pl-3 text-faint">~/.ssh/</span>
              <input
                bind:value={newName}
                class="min-w-0 flex-1 bg-transparent py-2 pr-3 text-fg outline-none"
                spellcheck="false"
                autocomplete="off"
                aria-invalid={nameError !== null}
              />
            </span>
          </label>
          {#if nameError}
            <p class="mt-1.5 text-xs text-status-crit">{$t(nameError, { name: newName.trim() })}</p>
          {:else}
            <p class="mt-1.5 text-xs text-faint">{$t('keysetup.keyNameHint', { name: newName.trim() })}</p>
          {/if}
        {:else}
          {#if publicDone}
            <div class="rounded-xl border border-status-ok/40 bg-surface-inset px-3.5 py-3">
              <p class="flex items-center gap-2 text-sm text-status-ok">
                <Icon name="check" size={14} />{$t('keysetup.publicDone')}
              </p>
              <p class="mt-1 text-xs text-muted">{$t('keysetup.publicDoneHint')}</p>
            </div>
          {:else}
            <p class="mb-2 text-xs text-faint">{$t('keysetup.publicHint')}</p>
            <label class="block space-y-1.5">
              <span class="text-xs font-medium text-muted">{$t('keysetup.publicLabel')}</span>
              <textarea
                bind:value={publicKey}
                rows="5"
                maxlength="16384"
                spellcheck="false"
                autocomplete="off"
                placeholder="ssh-ed25519 AAAA… name@computer"
                class="w-full resize-y rounded-lg bg-surface-inset px-3 py-2 font-mono text-xs text-fg outline-none focus-visible:ring-2 focus-visible:ring-focus"
              ></textarea>
            </label>
            <p class="mt-1.5 text-[11px] text-faint">{$t('keysetup.publicSafety')}</p>
            {#if publicError}<p class="mt-2 text-xs text-status-crit">{publicError}</p>{/if}
          {/if}
        {/if}
      </section>

      {#if tab !== 'public'}
      <section class="space-y-2">
        <h3 class={section}>{$t('keysetup.modeSection')}</h3>
        <label class="{radio} {cardState(mode === 'keyAndPassword')}">
          <input type="radio" class="mt-0.5 accent-current" bind:group={mode} value="keyAndPassword" />
          <span class="min-w-0">
            <span class="block text-sm">{$t('keysetup.modeBoth')}</span>
            <span class="block text-xs text-faint">{$t('keysetup.modeBothHint')}</span>
          </span>
        </label>
        <label class="{radio} {cardState(mode === 'keyOnly')}">
          <input type="radio" class="mt-0.5 accent-current" bind:group={mode} value="keyOnly" />
          <span class="min-w-0">
            <span class="flex items-center gap-1.5 text-sm">
              <Icon name="shield" size={13} />
              {$t('keysetup.modeKeyOnly')}
            </span>
            <span class="block text-xs text-faint">{$t('keysetup.modeKeyOnlyHint')}</span>
          </span>
        </label>
      </section>
      {/if}
    </div>

    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      {#if publicDone}
        <Button variant="primary" onclick={onClose}>{$t('common.done')}</Button>
      {:else}
        <Button variant="ghost" onclick={onClose}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="submit" disabled={!canStart}>
          {tab === 'public' ? $t('keysetup.addPublic') : $t('keysetup.install')}
        </Button>
      {/if}
    </footer>
  </form>
</Modal>
