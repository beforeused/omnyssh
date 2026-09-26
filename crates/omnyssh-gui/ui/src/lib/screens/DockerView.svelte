<script lang="ts">
  // A Docker tab: the containers of one host with live CPU/memory, start / stop /
  // restart / pause / remove, their logs, and a shell inside one (a terminal tab that
  // runs `docker exec`). Kept mounted while open, like the other session tabs; it
  // only polls while visible. Semantic tokens only.
  import { onDestroy, untrack } from 'svelte';
  import { Icon, StatusDot, type Status } from '$lib/theme';
  import Modal from '$lib/components/Modal.svelte';
  import { t } from '$lib/i18n';
  import type { ContainerDto, DockerActionDto } from '$lib/bindings';
  import { sessions, type Session } from '$lib/stores/sessions';
  import { spawnSession } from '$lib/stores/navigation';
  import { lastError } from '$lib/stores/notifications';
  import { dockerList, dockerAction, dockerLogs, dockerShellCommand } from '$lib/ipc/commands';

  let { session, active }: { session: Session; active: boolean } = $props();

  const REFRESH_MS = 5000;

  let containers = $state<ContainerDto[]>([]);
  let sudo = $state(false);
  let loaded = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let filter = $state('');
  let autoRefresh = $state(true);
  /** Container ids with an action in flight. */
  let busy = $state<Set<string>>(new Set());
  let confirmRemove = $state<ContainerDto | null>(null);
  let logs = $state<{ c: ContainerDto; text: string; tail: number; follow: boolean; loading: boolean } | null>(
    null
  );
  let logsEl = $state<HTMLPreElement>();

  const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));

  const shown = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    return q
      ? containers.filter((c) => `${c.name} ${c.image} ${c.status} ${c.ports}`.toLowerCase().includes(q))
      : containers;
  });
  const running = $derived(containers.filter((c) => c.state === 'running').length);

  async function refresh(): Promise<void> {
    if (loading) return;
    loading = true;
    try {
      const list = await dockerList(session.hostName);
      containers = Array.isArray(list?.containers) ? list.containers : [];
      sudo = !!list?.sudo;
      error = null;
      sessions.setStatus(session.id, 'connected');
    } catch (e) {
      error = message(e);
      sessions.setStatus(session.id, 'failed');
    } finally {
      loading = false;
      loaded = true;
    }
  }

  // Poll while the tab is visible (and auto-refresh is on).
  // `refresh` runs untracked: it reads and writes `loading`, which would otherwise make
  // this effect re-run (and re-fetch) every time a fetch settles.
  $effect(() => {
    if (!active) return;
    untrack(() => void refresh());
    if (!autoRefresh) return;
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => clearInterval(timer);
  });

  async function act(c: ContainerDto, action: DockerActionDto): Promise<void> {
    busy = new Set(busy).add(c.id);
    try {
      await dockerAction(session.hostName, c.id, action);
    } catch (e) {
      lastError.set(message(e));
    } finally {
      const next = new Set(busy);
      next.delete(c.id);
      busy = next;
      void refresh();
    }
  }

  async function openShell(c: ContainerDto): Promise<void> {
    try {
      const command = await dockerShellCommand(c.name || c.id, sudo);
      spawnSession('terminal', session.hostName, {
        initialInput: `${command}\n`,
        label: `${session.hostName} · ${c.name}`
      });
    } catch (e) {
      lastError.set(message(e));
    }
  }

  async function loadLogs(): Promise<void> {
    if (!logs) return;
    const current = logs;
    logs = { ...current, loading: true };
    try {
      const text = await dockerLogs(session.hostName, current.c.id, current.tail);
      if (logs && logs.c.id === current.c.id) {
        logs = { ...logs, text: text ?? '', loading: false };
        requestAnimationFrame(() => logsEl?.scrollTo({ top: logsEl.scrollHeight }));
      }
    } catch (e) {
      if (logs) logs = { ...logs, text: message(e), loading: false };
    }
  }

  function openLogs(c: ContainerDto): void {
    logs = { c, text: '', tail: 500, follow: false, loading: true };
    void loadLogs();
  }

  // "Follow" re-reads the tail every few seconds.
  $effect(() => {
    if (!logs?.follow || !active) return;
    const timer = setInterval(() => void loadLogs(), 3000);
    return () => clearInterval(timer);
  });

  onDestroy(() => {
    logs = null;
  });

  function dot(state: string): Status {
    if (state === 'running') return 'ok';
    if (state === 'paused' || state === 'restarting') return 'warn';
    if (state === 'dead') return 'crit';
    return 'off';
  }

  const pill =
    'inline-flex items-center gap-1 rounded-full border border-default px-2 py-0.5 text-[11px] ' +
    'text-muted transition hover:border-strong hover:bg-accent hover:text-accent-fg disabled:opacity-40 ' +
    'disabled:hover:bg-transparent disabled:hover:text-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const iconBtn =
    'grid h-7 w-7 place-items-center rounded-lg text-muted transition hover:bg-surface-inset ' +
    'hover:text-fg disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
</script>

<div class="absolute inset-0 flex flex-col bg-bg pt-[var(--titlebar-h)] {active ? '' : 'hidden'}">
  <section class="flex min-h-0 flex-1 flex-col px-6 pb-6 pt-3">
    <div class="mb-4 flex flex-wrap items-center gap-3">
      <h1 class="flex items-center gap-2 text-lg font-semibold tracking-tight">
        <Icon name="docker" size={18} />
        {$t('docker.title', { host: session.hostName })}
      </h1>
      {#if loaded && !error}
        <span class="text-xs text-muted">
          {$t('docker.running', { count: running })}
          {$t('docker.total', { count: containers.length })}
          {#if sudo}· {$t('docker.sudo')}{/if}
        </span>
      {/if}
      <div class="ml-auto flex items-center gap-2">
        <input
          bind:value={filter}
          placeholder={$t('docker.filter')}
          aria-label={$t('docker.filter')}
          class="w-48 rounded-full bg-surface-inset px-3 py-1.5 text-sm text-fg outline-none placeholder:text-faint focus-visible:ring-2 focus-visible:ring-focus"
        />
        <label class="flex cursor-pointer items-center gap-1.5 text-xs text-muted">
          <input type="checkbox" class="accent-current" bind:checked={autoRefresh} />
          {$t('docker.autoRefresh')}
        </label>
        <button
          type="button"
          class="grid h-8 w-8 place-items-center rounded-full border border-default text-muted transition hover:border-strong hover:bg-accent hover:text-accent-fg"
          title={$t('common.refresh')}
          aria-label={$t('common.refresh')}
          onclick={() => void refresh()}
        >
          <span class="inline-flex {loading ? 'animate-spin' : ''}"><Icon name="refresh" size={14} /></span>
        </button>
      </div>
    </div>

    {#if error}
      <p class="rounded-xl border border-default bg-surface px-4 py-3 text-sm text-status-crit">{error}</p>
    {:else if !loaded}
      <p class="py-10 text-center text-sm text-muted">{$t('docker.loading')}</p>
    {:else if containers.length === 0}
      <p class="py-10 text-center text-sm text-muted">{$t('docker.empty')}</p>
    {:else}
      <div class="min-h-0 flex-1 overflow-auto rounded-2xl border border-default bg-surface">
        <table class="w-full text-sm">
          <thead class="sticky top-0 bg-surface text-left text-[11px] uppercase tracking-wider text-faint">
            <tr class="border-b border-default">
              <th class="px-4 py-2.5 font-medium">{$t('docker.col.name')}</th>
              <th class="px-3 py-2.5 font-medium">{$t('docker.col.status')}</th>
              <th class="px-3 py-2.5 text-right font-medium">{$t('docker.col.cpu')}</th>
              <th class="px-3 py-2.5 text-right font-medium">{$t('docker.col.mem')}</th>
              <th class="px-3 py-2.5 font-medium">{$t('docker.col.ports')}</th>
              <th class="px-4 py-2.5"></th>
            </tr>
          </thead>
          <tbody>
            {#each shown as c (c.id)}
              {@const isBusy = busy.has(c.id)}
              <tr class="border-b border-default last:border-0">
                <td class="max-w-[16rem] px-4 py-2.5">
                  <div class="flex min-w-0 items-center gap-2">
                    <StatusDot status={dot(c.state)} label={c.state} />
                    <span class="truncate font-medium" title={c.name}>{c.name}</span>
                  </div>
                  <div class="truncate pl-4 font-mono text-[11px] text-faint" title={c.image}>{c.image}</div>
                </td>
                <td class="whitespace-nowrap px-3 py-2.5 text-xs text-muted">{c.status}</td>
                <td class="whitespace-nowrap px-3 py-2.5 text-right font-mono text-xs tabular-nums text-muted">
                  {c.cpu ?? ''}
                </td>
                <td class="whitespace-nowrap px-3 py-2.5 text-right font-mono text-xs tabular-nums text-muted">
                  {c.memory ?? ''}
                </td>
                <td class="max-w-[14rem] truncate px-3 py-2.5 font-mono text-[11px] text-faint" title={c.ports}>
                  {c.ports}
                </td>
                <td class="px-4 py-2.5">
                  <div class="flex items-center justify-end gap-1">
                    {#if c.state === 'running'}
                      <button type="button" class={pill} disabled={isBusy} onclick={() => void openShell(c)} title={$t('docker.shellTitle')}>
                        <Icon name="terminal" size={11} />
                        {$t('docker.shell')}
                      </button>
                    {/if}
                    <button type="button" class={pill} onclick={() => openLogs(c)}>
                      <Icon name="list" size={11} />
                      {$t('docker.logs')}
                    </button>
                    {#if c.state === 'running'}
                      <button type="button" class={iconBtn} disabled={isBusy} title={$t('docker.restart')} aria-label="{$t('docker.restart')} {c.name}" onclick={() => void act(c, 'restart')}>
                        <Icon name="refresh" size={14} />
                      </button>
                      <button type="button" class={iconBtn} disabled={isBusy} title={$t('docker.pause')} aria-label="{$t('docker.pause')} {c.name}" onclick={() => void act(c, 'pause')}>
                        <Icon name="pause" size={14} />
                      </button>
                      <button type="button" class={iconBtn} disabled={isBusy} title={$t('docker.stop')} aria-label="{$t('docker.stop')} {c.name}" onclick={() => void act(c, 'stop')}>
                        <Icon name="stop" size={14} />
                      </button>
                    {:else if c.state === 'paused'}
                      <button type="button" class={iconBtn} disabled={isBusy} title={$t('docker.unpause')} aria-label="{$t('docker.unpause')} {c.name}" onclick={() => void act(c, 'unpause')}>
                        <Icon name="play" size={14} />
                      </button>
                    {:else}
                      <button type="button" class={iconBtn} disabled={isBusy} title={$t('docker.start')} aria-label="{$t('docker.start')} {c.name}" onclick={() => void act(c, 'start')}>
                        <Icon name="play" size={14} />
                      </button>
                    {/if}
                    <button type="button" class={iconBtn} disabled={isBusy} title={$t('docker.remove')} aria-label="{$t('docker.remove')} {c.name}" onclick={() => (confirmRemove = c)}>
                      <Icon name="trash" size={14} />
                    </button>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </section>
</div>

{#if active && confirmRemove}
  {@const c = confirmRemove}
  <Modal label={$t('docker.removeTitle')} onClose={() => (confirmRemove = null)}>
    <header class="border-b border-default px-5 py-3.5">
      <h2 class="text-sm font-semibold">{$t('docker.removeTitle')}</h2>
    </header>
    <p class="px-5 py-4 text-sm text-muted">{$t('docker.removeBody', { name: c.name })}</p>
    <footer class="flex justify-end gap-2 border-t border-default px-5 py-3">
      <button type="button" class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg" onclick={() => (confirmRemove = null)}>
        {$t('common.cancel')}
      </button>
      <button
        type="button"
        class="rounded-full bg-accent px-5 py-2 text-sm font-medium text-accent-fg transition hover:opacity-90"
        onclick={() => {
          // Read before closing: `c` follows `confirmRemove`.
          const target = c;
          confirmRemove = null;
          void act(target, 'remove');
        }}
      >
        {$t('docker.remove')}
      </button>
    </footer>
  </Modal>
{/if}

{#if active && logs}
  <Modal label={$t('docker.logsTitle', { name: logs.c.name })} onClose={() => (logs = null)}>
    <header class="flex items-center gap-3 border-b border-default px-5 py-3.5">
      <h2 class="min-w-0 flex-1 truncate text-sm font-semibold">{$t('docker.logsTitle', { name: logs.c.name })}</h2>
      <div class="flex items-center gap-1 text-xs text-muted" role="group" aria-label={$t('docker.lines')}>
        {$t('docker.lines')}
        {#each [200, 500, 2000, 10000] as n (n)}
          <button
            type="button"
            class="rounded-md px-1.5 py-0.5 tabular-nums transition {logs.tail === n
              ? 'bg-accent text-accent-fg'
              : 'bg-surface-inset hover:text-fg'}"
            aria-pressed={logs.tail === n}
            onclick={() => {
              if (logs) logs = { ...logs, tail: n };
              void loadLogs();
            }}
          >
            {n}
          </button>
        {/each}
      </div>
      <label class="flex cursor-pointer items-center gap-1.5 text-xs text-muted">
        <input type="checkbox" class="accent-current" checked={logs.follow} onchange={() => logs && (logs = { ...logs, follow: !logs.follow })} />
        {$t('docker.follow')}
      </label>
      <button type="button" class="grid h-7 w-7 place-items-center rounded-lg text-muted hover:bg-surface-inset hover:text-fg" title={$t('common.refresh')} aria-label={$t('common.refresh')} onclick={() => void loadLogs()}>
        <span class="inline-flex {logs.loading ? 'animate-spin' : ''}"><Icon name="refresh" size={13} /></span>
      </button>
    </header>
    <pre
      bind:this={logsEl}
      class="max-h-[60vh] min-h-[12rem] select-text overflow-auto whitespace-pre-wrap break-all px-5 py-3 font-mono text-[11px] leading-relaxed text-muted">{logs.text ||
        (logs.loading ? $t('common.loading') : $t('docker.noLogs'))}</pre>
    <footer class="flex justify-end border-t border-default px-5 py-3">
      <button type="button" class="rounded-full px-4 py-2 text-sm text-muted transition hover:bg-surface-inset hover:text-fg" onclick={() => (logs = null)}>
        {$t('common.close')}
      </button>
    </footer>
  </Modal>
{/if}
