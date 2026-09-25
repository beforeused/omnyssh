<script lang="ts">
  // Picks an SSH private key: the keys found in ~/.ssh, plus a folder button that
  // opens the OS file picker for a key kept anywhere else. `value` is a path, or ''
  // for "no specific key" (whose label the owner supplies — "default key", "none").
  // Semantic tokens only.
  import { onMount } from 'svelte';
  import { Icon } from '$lib/theme';
  import Select from './Select.svelte';
  import { t } from '$lib/i18n';
  import { sshKeys, refreshKeys, keyFileName, browseForKey } from '$lib/stores/keys';
  import { inspectSshKey } from '$lib/ipc/commands';
  import type { SshKeyDto } from '$lib/bindings';

  let {
    value = $bindable(''),
    emptyLabel,
    allowEmpty = true,
    id,
    onChange
  }: {
    value?: string;
    /** Label of the '' choice; with `allowEmpty` false there is no such choice. */
    emptyLabel?: string;
    allowEmpty?: boolean;
    id?: string;
    onChange?: (path: string) => void;
  } = $props();

  let error = $state<string | null>(null);
  /** A key picked by hand that is not in ~/.ssh — kept listed while selected. */
  let extra = $state<SshKeyDto | null>(null);

  onMount(() => {
    void refreshKeys();
  });

  const listed = $derived.by(() => {
    const keys = [...$sshKeys];
    if (extra && !keys.some((k) => k.path === extra?.path)) keys.push(extra);
    if (value && !keys.some((k) => k.path === value)) {
      keys.push({ path: value, name: keyFileName(value), encrypted: false });
    }
    return keys;
  });

  let selectValue = $state('');
  $effect(() => {
    selectValue = value;
  });
  $effect(() => {
    const picked = selectValue;
    if (picked !== value) set(picked);
  });

  function set(path: string): void {
    value = path;
    error = null;
    onChange?.(path);
  }

  function label(k: SshKeyDto): string {
    const bits = [k.kind, k.comment, k.encrypted ? $t('keys.encrypted') : null].filter(Boolean);
    return bits.length ? `${k.name} — ${bits.join(', ')}` : k.name;
  }

  async function browse(): Promise<void> {
    const path = await browseForKey($t('keys.pickTitle'));
    if (!path) return;
    try {
      extra = await inspectSshKey(path);
      set(extra.path);
    } catch {
      error = $t('keys.notAKey', { path });
    }
  }

  const field =
    'w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none ' +
    'focus-visible:ring-2 focus-visible:ring-focus';
</script>

<div class="space-y-1">
  <div class="flex items-center gap-2">
    <div class="min-w-0 flex-1">
      <Select bind:value={selectValue} class="{field} font-mono text-xs">
        {#if allowEmpty}
          <option value="">{emptyLabel ?? $t('keys.none')}</option>
        {:else if !value}
          <option value="" disabled>{$t('keys.chooseKey')}</option>
        {/if}
        {#each listed as k (k.path)}
          <option value={k.path} title={k.path}>{label(k)}</option>
        {/each}
      </Select>
    </div>
    <button
      type="button"
      {id}
      class="grid h-9 w-9 shrink-0 place-items-center rounded-lg border border-default text-muted transition
        hover:border-strong hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
      title={$t('keys.browse')}
      aria-label={$t('keys.browse')}
      onclick={() => void browse()}
    >
      <Icon name="folder" size={15} />
    </button>
  </div>
  {#if value}
    <p class="truncate font-mono text-[11px] text-faint" title={value}>{value}</p>
  {:else if $sshKeys.length === 0}
    <p class="text-[11px] text-faint">{$t('keys.noneFound')}</p>
  {/if}
  {#if error}
    <p class="text-xs text-status-crit">{error}</p>
  {/if}
</div>
