<script lang="ts">
  // Settings (tech-gui.md §4.3): appearance (theme, language), privacy, the dashboard
  // refresh interval, files (editor, parallel transfer connections), SSH keys (the
  // app-wide default key), and an about block for this fork. The default key persists
  // to the shared config; the rest are frontend prefs (tauri-plugin-store).
  import { onMount, untrack } from 'svelte';
  import type { EditorAppDto } from '$lib/bindings';
  import { Surface, Icon } from '$lib/theme';
  import { theme } from '$lib/stores/theme';
  import { streamerMode } from '$lib/stores/streamer';
  import {
    refreshInterval,
    REFRESH_OPTIONS,
    editor,
    transferStreams,
    STREAM_OPTIONS
  } from '$lib/stores/settings';
  import Select from '$lib/components/Select.svelte';
  import KeyPicker from '$lib/components/KeyPicker.svelte';
  import { openExternal } from '$lib/ipc/openExternal';
  import { lastError } from '$lib/stores/notifications';
  import { defaultKey, loadDefaultKey, setDefaultKey } from '$lib/stores/keys';
  import { t, locale, LOCALES } from '$lib/i18n';
  import { detectEditors } from '$lib/ipc/commands';

  const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));
  const formatInterval = (secs: number): string => (secs < 60 ? `${secs}s` : `${secs / 60}m`);

  let version = $state<string | null>(null);

  onMount(() => {
    void detectEditors()
      .then((found) => (installed = Array.isArray(found) ? found : []))
      .catch(() => {});
    void loadDefaultKey();
    void import('@tauri-apps/api/app')
      .then(({ getVersion }) => getVersion())
      .then((v) => (version = v))
      .catch(() => {});
  });

  // -- About: this build is a fork --------------------------------------------
  const DEVELOPER = 'beforeused';
  const TELEGRAM_URL = 'https://t.me/beforeused';
  const ORIGINAL_URL = 'https://github.com/timhartmann7/omnyssh';

  async function open(url: string): Promise<void> {
    try {
      await openExternal(url);
    } catch (e) {
      lastError.set(message(e));
    }
  }

  // -- SSH keys: the app-wide default key ---------------------------------------
  let defaultKeyValue = $state('');
  $effect(() => {
    defaultKeyValue = $defaultKey ?? '';
  });
  async function changeDefaultKey(path: string): Promise<void> {
    try {
      await setDefaultKey(path || null);
    } catch (e) {
      lastError.set(message(e));
    }
  }

  // -- Files: which editor opens files ---------------------------------------
  let installed = $state<EditorAppDto[]>([]);
  // The select's value: 'system', 'app:<path>', 'command', or the 'choose' action.
  const editorKey = $derived(
    $editor.kind === 'app' ? `app:${$editor.path}` : $editor.kind === 'command' ? 'command' : 'system'
  );
  // The chosen app may not be in the detected list (picked by hand): list it anyway.
  const appOptions = $derived.by(() => {
    const list = [...installed];
    const current = $editor;
    if (current.kind === 'app' && !list.some((a) => a.path === current.path)) {
      list.push({ name: current.name, path: current.path });
    }
    return list;
  });
  let commandDraft = $state($editor.kind === 'command' ? $editor.command : 'code --wait {file}');
  let selectValue = $state('system');
  // Store → select, and select → store when the user picks something else.
  $effect(() => {
    selectValue = editorKey;
  });
  $effect(() => {
    const picked = selectValue;
    if (picked !== untrack(() => editorKey)) void onEditorChange(picked);
  });

  async function onEditorChange(value: string): Promise<void> {
    if (value === 'system') {
      editor.set({ kind: 'system' });
    } else if (value === 'command') {
      editor.set({ kind: 'command', command: commandDraft.trim() || 'code --wait {file}' });
    } else if (value.startsWith('app:')) {
      const path = value.slice(4);
      const app = appOptions.find((a) => a.path === path);
      editor.set({ kind: 'app', path, name: app?.name ?? path });
    } else if (value === 'choose') {
      selectValue = editorKey;
      await chooseApp();
    }
  }

  async function chooseApp(): Promise<void> {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const isMac = navigator.userAgent.includes('Mac');
      const picked = await open({
        title: $t('settings.chooseAppTitle'),
        multiple: false,
        directory: false,
        defaultPath: isMac ? '/Applications' : undefined
      });
      if (typeof picked !== 'string' || !picked) return;
      const name = (picked.split(/[\\/]/).pop() ?? picked).replace(/\.(app|exe)$/i, '');
      editor.set({ kind: 'app', path: picked, name });
    } catch (e) {
      lastError.set(message(e));
    }
  }

  function saveCommand(): void {
    const command = commandDraft.trim();
    if (command) editor.set({ kind: 'command', command });
  }

  const seg =
    'rounded-lg px-3 py-1.5 text-sm transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus';
  const segState = (active: boolean): string =>
    active ? 'bg-accent text-accent-fg' : 'text-muted hover:bg-surface-inset hover:text-fg';
</script>

<section class="mx-auto h-full max-w-2xl p-6">
  <h1 class="mb-5 text-lg font-semibold tracking-tight">{$t('settings.title')}</h1>

  <div class="space-y-4">
    <!-- Appearance -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">{$t('settings.appearance')}</h2>
      <div class="space-y-4">
        <div class="flex items-center justify-between gap-4">
          <div>
            <p class="text-sm">{$t('settings.theme')}</p>
            <p class="text-xs text-muted">{$t('settings.themeHint')}</p>
          </div>
          <div class="flex gap-1 rounded-xl bg-surface-inset p-1">
            <button
              type="button"
              class="{seg} {segState($theme === 'light')}"
              aria-pressed={$theme === 'light'}
              onclick={() => theme.set('light')}
            >
              <span class="flex items-center gap-1.5"><Icon name="sun" size={14} /> {$t('settings.light')}</span>
            </button>
            <button
              type="button"
              class="{seg} {segState($theme === 'dark')}"
              aria-pressed={$theme === 'dark'}
              onclick={() => theme.set('dark')}
            >
              <span class="flex items-center gap-1.5"><Icon name="moon" size={14} /> {$t('settings.dark')}</span>
            </button>
          </div>
        </div>
        <div class="flex items-center justify-between gap-4 border-t border-default pt-4">
          <div>
            <p class="text-sm">{$t('settings.language')}</p>
            <p class="text-xs text-muted">{$t('settings.languageHint')}</p>
          </div>
          <div class="flex shrink-0 gap-1 rounded-xl bg-surface-inset p-1">
            {#each LOCALES as l (l.id)}
              <button
                type="button"
                class="{seg} {segState($locale === l.id)}"
                aria-pressed={$locale === l.id}
                lang={l.id}
                onclick={() => locale.set(l.id)}
              >
                {l.label}
              </button>
            {/each}
          </div>
        </div>
      </div>
    </Surface>

    <!-- Privacy -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">{$t('settings.privacy')}</h2>
      <div class="flex items-center justify-between gap-4">
        <div class="min-w-0">
          <p class="text-sm">{$t('settings.streamer')}</p>
          <p class="text-xs text-muted">{$t('settings.streamerHint')}</p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={$streamerMode}
          aria-label={$t('settings.streamer')}
          onclick={() => streamerMode.toggle()}
          class="relative h-6 w-11 shrink-0 rounded-full transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus {$streamerMode
            ? 'bg-accent'
            : 'bg-surface-inset'}"
        >
          <span
            class="absolute top-0.5 h-5 w-5 rounded-full bg-surface shadow-soft transition-[left] {$streamerMode
              ? 'left-[1.375rem]'
              : 'left-0.5'}"
          ></span>
        </button>
      </div>
    </Surface>

    <!-- Dashboard -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">{$t('settings.dashboard')}</h2>
      <div class="flex items-center justify-between gap-4">
        <div>
          <p class="text-sm">{$t('settings.refresh')}</p>
          <p class="text-xs text-muted">{$t('settings.refreshHint')}</p>
        </div>
        <div class="flex flex-wrap justify-end gap-1 rounded-xl bg-surface-inset p-1">
          {#each REFRESH_OPTIONS as secs (secs)}
            <button
              type="button"
              class="{seg} tabular-nums {segState($refreshInterval === secs)}"
              aria-pressed={$refreshInterval === secs}
              onclick={() => refreshInterval.set(secs)}
            >
              {formatInterval(secs)}
            </button>
          {/each}
        </div>
      </div>
    </Surface>

    <!-- Files -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">{$t('settings.files')}</h2>
      <div class="space-y-4">
        <div class="flex items-center justify-between gap-4">
          <div class="min-w-0">
            <p class="text-sm">{$t('settings.openWith')}</p>
            <p class="text-xs text-muted">{$t('settings.openWithHint')}</p>
          </div>
          <div class="w-56 shrink-0">
            <Select
              bind:value={selectValue}
              class="w-full rounded-lg bg-surface-inset px-3 py-2 text-sm text-fg outline-none focus-visible:ring-2 focus-visible:ring-focus"
            >
              <option value="system">{$t('settings.systemDefault')}</option>
              {#each appOptions as app (app.path)}
                <option value="app:{app.path}">{app.name}</option>
              {/each}
              <option value="command">{$t('settings.customCommand')}</option>
              <option value="choose">{$t('settings.chooseApp')}</option>
            </Select>
          </div>
        </div>
        {#if $editor.kind === 'command'}
          <form
            class="flex items-center gap-2"
            onsubmit={(e) => {
              e.preventDefault();
              saveCommand();
            }}
          >
            <input
              bind:value={commandDraft}
              onblur={saveCommand}
              aria-label={$t('settings.editorCommand')}
              placeholder={'code --wait {file}'}
              class="min-w-0 flex-1 rounded-lg bg-surface-inset px-3 py-2 font-mono text-xs text-fg outline-none placeholder:text-faint focus-visible:ring-2 focus-visible:ring-focus"
            />
          </form>
          <p class="-mt-2 text-xs text-muted">{$t('settings.commandHint')}</p>
        {/if}

        <div class="flex items-center justify-between gap-4 border-t border-default pt-4">
          <div class="min-w-0">
            <p class="text-sm">{$t('settings.streams')}</p>
            <p class="text-xs text-muted">{$t('settings.streamsHint')}</p>
          </div>
          <div class="flex shrink-0 gap-1 rounded-xl bg-surface-inset p-1">
            {#each STREAM_OPTIONS as n (n)}
              <button
                type="button"
                class="{seg} tabular-nums {segState($transferStreams === n)}"
                aria-pressed={$transferStreams === n}
                onclick={() => transferStreams.set(n)}
              >
                {n}
              </button>
            {/each}
          </div>
        </div>
      </div>
    </Surface>

    <!-- SSH keys -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">{$t('settings.keys')}</h2>
      <div class="space-y-2">
        <div>
          <p class="text-sm">{$t('settings.defaultKey')}</p>
          <p class="text-xs text-muted">{$t('settings.defaultKeyHint')}</p>
        </div>
        <KeyPicker
          bind:value={defaultKeyValue}
          emptyLabel={$t('keys.noDefault')}
          onChange={(path) => void changeDefaultKey(path)}
        />
      </div>
    </Surface>

    <!-- About -->
    <Surface class="p-5">
      <h2 class="mb-3 text-sm font-semibold">{$t('about.title')}</h2>
      <div class="space-y-4">
        <p class="text-sm">{$t('about.fork')}</p>
        <dl class="grid grid-cols-[auto_1fr] items-center gap-x-6 gap-y-2.5 border-t border-default pt-4 text-sm">
          <dt class="text-muted">{$t('about.developer')}</dt>
          <dd class="font-medium">{DEVELOPER}</dd>
          <dt class="text-muted">{$t('about.contact')}</dt>
          <dd>
            <button
              type="button"
              class="inline-flex items-center gap-1.5 font-mono text-fg underline decoration-faint underline-offset-4 transition hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
              title={TELEGRAM_URL}
              onclick={() => void open(TELEGRAM_URL)}
            >
              <Icon name="telegram" size={13} />
              @{DEVELOPER}
            </button>
          </dd>
          <dt class="text-muted">{$t('about.original')}</dt>
          <dd>
            <button
              type="button"
              class="font-mono text-xs text-muted underline decoration-faint underline-offset-4 transition hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus"
              onclick={() => void open(ORIGINAL_URL)}
            >
              github.com/timhartmann7/omnyssh
            </button>
          </dd>
          {#if version}
            <dt class="text-muted">{$t('about.version')}</dt>
            <dd class="font-mono text-xs text-muted">{version}</dd>
          {/if}
        </dl>
      </div>
    </Surface>
  </div>
</section>
