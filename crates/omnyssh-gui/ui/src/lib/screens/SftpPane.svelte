<script lang="ts" module>
  export type SelectMode = 'only' | 'toggle' | 'range';
  /** Keyboard shortcuts the pane hands to its owner. */
  export type PaneKey = 'delete' | 'rename' | 'refresh';
</script>

<script lang="ts">
  // One side of the dual-pane SFTP browser (tech-gui.md §3.2), as a file manager:
  // a breadcrumb path bar (click a segment, or click the bar / press Cmd+L to type
  // a path), bookmarks, a hidden-files toggle and a filter, then sortable Name / Size
  // / Modified columns. Click selects (Cmd/Ctrl toggles, Shift extends), double-click
  // opens, `..` goes up on a single click; Backspace goes up, Delete / F2 / F5 are
  // handed to the owner. Rows start drags and folders are drop targets; the parent
  // owns the drag itself. Semantic tokens only — no colour literals (§5.1).
  import type { Snippet } from 'svelte';
  import { tick } from 'svelte';
  import { Icon } from '$lib/theme';
  import type { FileEntryDto } from '$lib/bindings';
  import {
    formatBytes,
    breadcrumbs,
    permString,
    type Pane,
    type PaneSide,
    type SortKey,
    type SortSpec
  } from '$lib/stores/sftp';
  import { t, locale } from '$lib/i18n';

  let {
    title,
    side,
    pane,
    entries,
    sort,
    showHidden,
    filter = $bindable(''),
    bookmarked,
    dropTarget,
    onNavigate,
    onOpenPath,
    onUp,
    onOpen,
    onSelect,
    onSelectAll,
    onContextMenu,
    onRowPointerDown,
    onSort,
    onToggleHidden,
    onToggleBookmark,
    onShowBookmarks,
    onKey,
    toolbar
  }: {
    title: string;
    side: PaneSide;
    pane: Pane;
    /** What to list, already filtered and sorted (see `viewEntries`). */
    entries: FileEntryDto[];
    sort: SortSpec;
    showHidden: boolean;
    filter?: string;
    bookmarked: boolean;
    /** Set while a drag hovers this pane: the folder under the pointer, or null for
     *  the pane's current folder. */
    dropTarget?: { dir: string | null };
    onNavigate: (entry: FileEntryDto) => void;
    onOpenPath: (path: string) => void;
    onUp: () => void;
    onOpen: (entry: FileEntryDto) => void;
    onSelect: (path: string, mode: SelectMode) => void;
    onSelectAll: () => void;
    onContextMenu: (entry: FileEntryDto | null, x: number, y: number) => void;
    onRowPointerDown: (entry: FileEntryDto, e: PointerEvent) => void;
    onSort: (key: SortKey) => void;
    onToggleHidden: () => void;
    onToggleBookmark: () => void;
    onShowBookmarks: (x: number, y: number) => void;
    onKey: (key: PaneKey) => void;
    toolbar?: Snippet;
  } = $props();

  let editingPath = $state(false);
  let pathDraft = $state('');
  let pathInput = $state<HTMLInputElement>();
  let listEl = $state<HTMLDivElement>();

  const crumbs = $derived(breadcrumbs(pane.path));
  const dateFormat = $derived(
    new Intl.DateTimeFormat($locale, { dateStyle: 'short', timeStyle: 'short' })
  );

  async function editPath(): Promise<void> {
    pathDraft = pane.path;
    editingPath = true;
    await tick();
    pathInput?.select();
  }

  function submitPath(): void {
    const next = pathDraft.trim();
    editingPath = false;
    if (next && next !== pane.path) onOpenPath(next);
  }

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
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      onSelectAll();
    } else if (mod && e.key.toLowerCase() === 'l') {
      e.preventDefault();
      void editPath();
    } else if ((e.key === 'Backspace' && !mod) || (mod && e.key === 'ArrowUp')) {
      e.preventDefault();
      onUp();
    } else if (e.key === 'Delete' || (mod && e.key === 'Backspace')) {
      e.preventDefault();
      onKey('delete');
    } else if (e.key === 'F2') {
      e.preventDefault();
      onKey('rename');
    } else if (e.key === 'F5') {
      e.preventDefault();
      onKey('refresh');
    }
  }

  function rowContext(entry: FileEntryDto, e: MouseEvent): void {
    e.preventDefault();
    e.stopPropagation();
    if (entry.name !== '..' && !pane.marked.has(entry.path)) onSelect(entry.path, 'only');
    onContextMenu(entry.name === '..' ? null : entry, e.clientX, e.clientY);
  }

  function modified(entry: FileEntryDto): string {
    return entry.modified ? dateFormat.format(new Date(entry.modified * 1000)) : '';
  }

  function rowTitle(entry: FileEntryDto): string {
    const perms = permString(entry.permissions, entry.isDir);
    return perms ? `${entry.name}\n${perms}` : entry.name;
  }

  const rowBase =
    'flex w-full min-w-0 select-none items-center gap-2 rounded px-2 py-1.5 text-left text-sm ' +
    'transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const miniBtn =
    'grid h-6 w-6 shrink-0 place-items-center rounded-md text-faint transition hover:bg-surface-inset ' +
    'hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const colBtn =
    'inline-flex items-center gap-0.5 rounded px-1 py-0.5 text-[11px] uppercase tracking-wider transition ' +
    'hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const sortMark = (key: SortKey): string => (sort.key === key ? (sort.dir === 'asc' ? '↑' : '↓') : '');
</script>

<section
  aria-label={title}
  data-drop-side={side}
  class="relative flex min-h-0 min-w-0 flex-1 flex-col"
>
  <header class="shrink-0 space-y-1.5 border-b border-default px-3 pb-2 pt-2.5">
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

    <!-- Path bar: breadcrumbs, or a text field while typing a path. -->
    <div class="flex items-center gap-1">
      <button type="button" class={miniBtn} title={$t('fm.up')} aria-label={$t('fm.up')} onclick={onUp}>
        <Icon name="arrow-up" size={13} />
      </button>
      {#if editingPath}
        <input
          bind:this={pathInput}
          bind:value={pathDraft}
          aria-label={$t('fm.pathLabel')}
          spellcheck="false"
          autocomplete="off"
          class="min-w-0 flex-1 rounded-md bg-surface-inset px-2 py-1 font-mono text-xs text-fg outline-none focus-visible:ring-2 focus-visible:ring-focus"
          onkeydown={(e) => {
            if (e.key === 'Enter') submitPath();
            else if (e.key === 'Escape') editingPath = false;
          }}
          onblur={() => (editingPath = false)}
        />
      {:else}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="flex min-w-0 flex-1 cursor-text items-center overflow-hidden rounded-md px-1 py-0.5 font-mono text-xs text-faint hover:bg-surface-inset"
          title={$t('fm.editPath')}
          onclick={(e) => {
            if (e.target === e.currentTarget) void editPath();
          }}
        >
          {#each crumbs as crumb, i (crumb.path)}
            {#if i > 1}<span class="shrink-0 px-0.5">/</span>{/if}
            <button
              type="button"
              class="max-w-[12rem] shrink truncate rounded px-0.5 transition hover:text-fg
                {i === crumbs.length - 1 ? 'text-muted' : ''}"
              title={crumb.path}
              onclick={() => onOpenPath(crumb.path)}
            >
              {crumb.label}
            </button>
          {/each}
          {#if crumbs.length === 0}<span>—</span>{/if}
        </div>
      {/if}
      <button
        type="button"
        class="{miniBtn} {bookmarked ? 'text-fg' : ''}"
        title={bookmarked ? $t('fm.removeBookmark') : $t('fm.addBookmark')}
        aria-label={bookmarked ? $t('fm.removeBookmark') : $t('fm.addBookmark')}
        aria-pressed={bookmarked}
        onclick={onToggleBookmark}
      >
        <Icon name={bookmarked ? 'star' : 'bookmark'} size={13} />
      </button>
      <button
        type="button"
        class={miniBtn}
        title={$t('fm.bookmarks')}
        aria-label={$t('fm.bookmarks')}
        onclick={(e) => {
          const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
          onShowBookmarks(r.left, r.bottom + 4);
        }}
      >
        <Icon name="list" size={13} />
      </button>
      <button
        type="button"
        class={miniBtn}
        title={showHidden ? $t('fm.hideHidden') : $t('fm.showHidden')}
        aria-label={showHidden ? $t('fm.hideHidden') : $t('fm.showHidden')}
        aria-pressed={showHidden}
        onclick={onToggleHidden}
      >
        <Icon name={showHidden ? 'eye' : 'eye-off'} size={13} />
      </button>
    </div>

    <!-- Filter + column headers. -->
    <div class="flex items-center gap-2 pl-1">
      <input
        bind:value={filter}
        placeholder={$t('fm.filter')}
        aria-label={$t('fm.filterLabel')}
        spellcheck="false"
        class="w-28 min-w-0 rounded-md bg-surface-inset px-2 py-0.5 text-xs text-fg outline-none placeholder:text-faint focus:w-40 focus-visible:ring-2 focus-visible:ring-focus"
        onkeydown={(e) => {
          if (e.key === 'Escape') filter = '';
        }}
      />
      <div class="ml-auto flex items-center gap-2 text-faint">
        <button type="button" class={colBtn} title={$t('fm.sortBy', { column: $t('fm.name') })} onclick={() => onSort('name')}>
          {$t('fm.name')} {sortMark('name')}
        </button>
        <button type="button" class="{colBtn} w-14 justify-end" title={$t('fm.sortBy', { column: $t('fm.size') })} onclick={() => onSort('size')}>
          {$t('fm.size')} {sortMark('size')}
        </button>
        <button type="button" class="{colBtn} w-24 justify-end" title={$t('fm.sortBy', { column: $t('fm.modified') })} onclick={() => onSort('modified')}>
          {$t('fm.modified')} {sortMark('modified')}
        </button>
      </div>
    </div>
  </header>

  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    bind:this={listEl}
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
        {#each entries as entry, i (i)}
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
              title={rowTitle(entry)}
              onclick={(e) => click(entry, e)}
              ondblclick={() => !isParent && open(entry)}
              onkeydown={(e) => keydown(entry, e)}
              oncontextmenu={(e) => rowContext(entry, e)}
              onpointerdown={(e) => !isParent && onRowPointerDown(entry, e)}
            >
              <Icon name={entry.isDir ? 'folder' : 'file'} size={15} />
              <span class="min-w-0 flex-1 truncate {entry.isDir ? 'font-medium' : ''} {dropHere ? '' : entry.isDir ? 'text-fg' : ''}">
                {entry.name}{#if entry.isLink}<span class="ml-1 text-[10px] {dropHere ? '' : 'text-faint'}">↗</span>{/if}
              </span>
              <span class="w-14 shrink-0 text-right tabular-nums text-xs {dropHere ? '' : 'text-faint'}">
                {entry.isDir ? '' : formatBytes(entry.size)}
              </span>
              <span class="w-24 shrink-0 truncate text-right tabular-nums text-[11px] {dropHere ? '' : 'text-faint'}">
                {isParent ? '' : modified(entry)}
              </span>
            </button>
          </li>
        {/each}
      </ul>
      {#if entries.length === 0 || (entries.length === 1 && entries[0].name === '..')}
        {#if filter.trim()}
          <p class="px-2 py-6 text-center text-sm text-faint">{$t('fm.noMatch', { query: filter.trim() })}</p>
        {/if}
      {/if}
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
