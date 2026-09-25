<script lang="ts" module></script>

<script lang="ts">
  // One side of the dual-pane SFTP browser (tech-gui.md §3.2): a current-path header
  // with a parent-supplied toolbar, then the entry list. Click selects (Cmd/Ctrl-click
  // toggles, Shift-click extends), double-click opens — a folder navigates, a file
  // opens in the configured editor — and `..` navigates up on a single click. Rows
  // start drags and are drop targets (folders); the parent owns the drag itself.
  // Semantic tokens only — no colour literals (§5.1).
  import { t } from '$lib/i18n';
  import type { Snippet } from 'svelte';
  import { Icon } from '$lib/theme';
  import type { FileEntryDto } from '$lib/bindings';
  import { formatBytes, type Pane, type PaneSide } from '$lib/stores/sftp';

  export type SelectMode = 'only' | 'toggle' | 'range';

  let {
    title,
    side,
    pane,
    dropTarget,
    onNavigate,
    onOpen,
    onSelect,
    onSelectAll,
    onContextMenu,
    onRowPointerDown,
    toolbar
  }: {
    title: string;
    side: PaneSide;
    pane: Pane;
    /** Set while a drag hovers this pane: the folder under the pointer, or null for
     *  the pane's current folder. */
    dropTarget?: { dir: string | null };
    onNavigate: (entry: FileEntryDto) => void;
    onOpen: (entry: FileEntryDto) => void;
    onSelect: (path: string, mode: SelectMode) => void;
    onSelectAll: () => void;
    onContextMenu: (entry: FileEntryDto | null, x: number, y: number) => void;
    onRowPointerDown: (entry: FileEntryDto, e: PointerEvent) => void;
    toolbar?: Snippet;
  } = $props();

  function click(entry: FileEntryDto, e: MouseEvent): void {
    if (entry.name === '..') {
      onNavigate(entry);
    } else if (e.metaKey || e.ctrlKey) {
      onSelect(entry.path, 'toggle');
    } else if (e.shiftKey) {
      onSelect(entry.path, 'range');
    } else {
      onSelect(entry.path, 'only');
    }
  }

  function open(entry: FileEntryDto): void {
    if (entry.isDir) onNavigate(entry);
    else onOpen(entry);
  }

  function keydown(entry: FileEntryDto, e: KeyboardEvent): void {
    if (e.key === 'Enter') {
      e.preventDefault();
      open(entry);
    } else if (e.key === ' ' && entry.name !== '..') {
      e.preventDefault();
      onSelect(entry.path, 'toggle');
    }
  }

  function listKeydown(e: KeyboardEvent): void {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      onSelectAll();
    }
  }

  function rowContext(entry: FileEntryDto, e: MouseEvent): void {
    e.preventDefault();
    e.stopPropagation();
    if (entry.name !== '..' && !pane.marked.has(entry.path)) onSelect(entry.path, 'only');
    onContextMenu(entry.name === '..' ? null : entry, e.clientX, e.clientY);
  }

  const rowBase =
    'flex w-full min-w-0 select-none items-center gap-2 rounded px-2 py-1.5 text-left text-sm ' +
    'transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
</script>

<section
  aria-label={title}
  data-drop-side={side}
  class="relative flex min-h-0 min-w-0 flex-1 flex-col"
>
  <header class="shrink-0 border-b border-default px-3 py-2.5">
    <div class="flex items-center justify-between gap-2">
      <h2
        title={title}
        class="truncate text-xs font-semibold uppercase tracking-[0.14em] text-muted"
      >
        {title}
      </h2>
      <div class="flex shrink-0 items-center gap-1">
        {@render toolbar?.()}
      </div>
    </div>
    <div class="mt-1 truncate font-mono text-xs text-faint" title={pane.path}>
      {pane.path || '—'}
    </div>
  </header>

  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    role="listbox"
    aria-multiselectable="true"
    aria-label={$t('sftp.files', { title })}
    tabindex="-1"
    class="min-h-0 flex-1 overflow-y-auto px-1.5 py-1.5"
    onkeydown={listKeydown}
    oncontextmenu={(e) => {
      e.preventDefault();
      onContextMenu(null, e.clientX, e.clientY);
    }}
  >
    {#if pane.error}
      <p class="px-2 py-6 text-center text-sm text-status-crit">{pane.error}</p>
    {:else if pane.loading && pane.entries.length === 0}
      <p class="px-2 py-6 text-center text-sm text-faint">{$t('common.loading')}</p>
    {:else if pane.entries.length === 0}
      <p class="px-2 py-6 text-center text-sm text-faint">{$t('sftp.emptyDir')}</p>
    {:else}
      <ul class="space-y-0.5">
        {#each pane.entries as entry, i (i)}
          {@const isParent = entry.name === '..'}
          {@const marked = pane.marked.has(entry.path)}
          {@const dropHere = entry.isDir && dropTarget?.dir === entry.path}
          <li class="flex items-center gap-1.5">
            {#if isParent}
              <span class="h-4 w-4 shrink-0"></span>
            {:else}
              <button
                type="button"
                role="checkbox"
                aria-checked={marked}
                aria-label={$t('sftp.mark', { name: entry.name })}
                class="grid h-4 w-4 shrink-0 place-items-center rounded border transition
                  focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus
                  {marked ? 'border-accent bg-accent text-accent-fg' : 'border-strong text-transparent'}"
                onclick={() => onSelect(entry.path, 'toggle')}
              >
                {#if marked}<Icon name="check" size={11} />{/if}
              </button>
            {/if}
            <button
              type="button"
              role="option"
              aria-selected={marked}
              data-drop-dir={entry.isDir ? entry.path : undefined}
              class="{rowBase} {dropHere
                ? 'bg-accent text-accent-fg'
                : marked
                  ? 'bg-surface-inset text-fg'
                  : 'text-muted hover:bg-surface-inset hover:text-fg'}"
              title={entry.name}
              onclick={(e) => click(entry, e)}
              ondblclick={() => !isParent && open(entry)}
              onkeydown={(e) => keydown(entry, e)}
              oncontextmenu={(e) => rowContext(entry, e)}
              onpointerdown={(e) => !isParent && onRowPointerDown(entry, e)}
            >
              <Icon name={entry.isDir ? 'folder' : 'file'} size={15} />
              <span class="min-w-0 flex-1 truncate {entry.isDir ? 'font-medium' : ''} {dropHere ? '' : entry.isDir ? 'text-fg' : ''}">
                {entry.name}
              </span>
              {#if !entry.isDir}
                <span class="shrink-0 tabular-nums text-xs {dropHere ? '' : 'text-faint'}">{formatBytes(entry.size)}</span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  {#if dropTarget}
    <!-- The whole pane is the target when no folder row is under the pointer. -->
    <div
      class="pointer-events-none absolute inset-1 rounded-xl border-2 border-dashed
        {dropTarget.dir ? 'border-default' : 'border-strong'}"
      aria-hidden="true"
    ></div>
  {/if}
</section>
