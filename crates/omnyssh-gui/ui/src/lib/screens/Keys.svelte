<script lang="ts">
  import { get } from 'svelte/store';
  import { onMount } from 'svelte';
  import { Button, Icon, Surface } from '$lib/theme';
  import { t } from '$lib/i18n';
  import { hosts } from '$lib/stores/hosts';
  import { defaultKey, loadDefaultKey, refreshKeys, sshKeys } from '$lib/stores/keys';
  import { hostAuth } from '$lib/ipc/commands';
  import { lastError } from '$lib/stores/notifications';
  import KeyCreateDialog from './KeyCreateDialog.svelte';

  let createOpen = $state(false);
  let usages = $state<Record<string, string[]>>({});
  let copiedPath = $state<string | null>(null);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  function samePath(left: string | null, right: string): boolean {
    if (!left) return false;
    const a = left.replaceAll('\\', '/');
    const b = right.replaceAll('\\', '/');
    if (a === b) return true;
    return a.startsWith('~/.ssh/') && b.endsWith(`/.ssh/${a.slice('~/.ssh/'.length)}`);
  }

  async function refresh(): Promise<void> {
    await Promise.all([refreshKeys(), loadDefaultKey()]);
    const entries = await Promise.all(
      get(hosts).map(async (host) => ({ host: host.name, auth: await hostAuth(host.name) }))
    );
    const next: Record<string, string[]> = {};
    for (const key of get(sshKeys)) {
      const isDefault = samePath(get(defaultKey), key.path);
      next[key.path] = entries
        .filter(
          ({ auth }) =>
            samePath(auth.identityFile ?? null, key.path) || (!auth.identityFile && isDefault)
        )
        .map(({ host }) => host);
    }
    usages = next;
  }

  onMount(() => {
    void refresh().catch((reason) =>
      lastError.set(reason instanceof Error ? reason.message : String(reason))
    );
    return () => {
      if (copyTimer !== undefined) clearTimeout(copyTimer);
    };
  });

  async function copyPublicKey(path: string, publicKey: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(publicKey);
      copiedPath = path;
      if (copyTimer !== undefined) clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copiedPath = null), 1800);
    } catch (reason) {
      lastError.set(reason instanceof Error ? reason.message : String(reason));
    }
  }

  const badge = 'rounded-full border border-default px-2 py-0.5 text-[10px] text-faint';
</script>

<section class="min-h-full px-6 pb-8 pt-3">
  <div class="mb-5 flex items-center gap-3">
    <div>
      <h1 class="text-lg font-semibold tracking-tight">{$t('keys.page.title')}</h1>
      <p class="mt-1 text-xs text-muted">{$t('keys.page.subtitle')}</p>
    </div>
    <Button variant="primary" onclick={() => (createOpen = true)}>
      <Icon name="plus" size={13} />{$t('keys.create.action')}
    </Button>
  </div>

  {#if $sshKeys.length === 0}
    <Surface class="p-8 text-center">
      <p class="font-medium">{$t('keys.page.empty')}</p>
      <p class="mt-1 text-sm text-muted">{$t('keys.page.emptyHint')}</p>
    </Surface>
  {:else}
    <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(22rem,1fr))]">
      {#each $sshKeys as key (key.path)}
        <Surface class="flex min-w-0 flex-col gap-3 p-5">
          <div class="flex min-w-0 items-start gap-3">
            <span class="grid h-9 w-9 shrink-0 place-items-center rounded-full bg-surface-inset text-muted">
              <Icon name="key" size={16} />
            </span>
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-1.5">
                <h2 class="truncate font-mono text-sm font-semibold">{key.name}</h2>
                {#if key.kind}<span class={badge}>{key.kind}</span>{/if}
                {#if key.encrypted}<span class={badge}>{$t('keys.encrypted')}</span>{/if}
                {#if samePath($defaultKey, key.path)}<span class={badge}>{$t('keys.page.default')}</span>{/if}
              </div>
              <p class="mt-1 truncate text-xs text-faint" title={key.path}>{key.path}</p>
            </div>
          </div>

          <div>
            <p class="mb-1.5 text-xs font-medium text-muted">{$t('keys.page.usedBy')}</p>
            {#if (usages[key.path] ?? []).length > 0}
              <div class="flex flex-wrap gap-1.5">
                {#each usages[key.path] as host (host)}<span class={badge}>{host}</span>{/each}
              </div>
            {:else}
              <p class="text-xs text-faint">{$t('keys.page.notUsed')}</p>
            {/if}
          </div>

          <div class="mt-auto">
            <div class="mb-1.5 flex items-center justify-between gap-2">
              <p class="text-xs font-medium text-muted">{$t('keys.page.public')}</p>
              {#if key.publicKey}
                <button
                  type="button"
                  class="inline-flex items-center gap-1 rounded-full px-2 py-1 text-xs text-muted transition hover:bg-surface-inset hover:text-fg"
                  onclick={() => void copyPublicKey(key.path, key.publicKey!)}
                >
                  <Icon name={copiedPath === key.path ? 'check' : 'copy'} size={12} />
                  {copiedPath === key.path ? $t('keys.page.copied') : $t('keys.page.copy')}
                </button>
              {/if}
            </div>
            {#if key.publicKey}
              <textarea
                readonly
                rows="3"
                value={key.publicKey}
                aria-label={$t('keys.page.public')}
                class="w-full resize-none rounded-lg bg-surface-inset px-3 py-2 font-mono text-[11px] leading-4 text-muted outline-none"
              ></textarea>
            {:else}
              <p class="rounded-lg bg-surface-inset px-3 py-2 text-xs text-faint">
                {$t('keys.page.publicMissing')}
              </p>
            {/if}
          </div>
        </Surface>
      {/each}
    </div>
  {/if}
</section>

{#if createOpen}
  <KeyCreateDialog onClose={() => (createOpen = false)} />
{/if}
