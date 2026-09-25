<script lang="ts">
  // One live xterm bound to a remote PTY. Used full-screen by a terminal tab
  // (TerminalView) and docked under an SFTP tab (SftpView). Raw output arrives on a
  // per-session channel (§3.3/§3.6); keystrokes/resizes go back over the terminal
  // commands. Re-themes live with the theme store (§5.1). The owner hears about the
  // lifecycle through callbacks; everything xterm stays in here.
  import '@xterm/xterm/css/xterm.css';
  import { onMount, onDestroy } from 'svelte';
  import type { Terminal } from '@xterm/xterm';
  import type { FitAddon } from '@xterm/addon-fit';
  import { Channel } from '@tauri-apps/api/core';
  import { theme } from '$lib/stores/theme';
  import { xtermTheme } from '$lib/theme/terminalTheme';
  import { terminalOpen, terminalWrite, terminalResize, terminalClose } from '$lib/ipc/commands';
  import { shouldFadeTop } from './terminalFade';
  import { chunkBytes } from './terminalInput';
  import type { TerminalBytes } from '$lib/bindings';

  let {
    hostName,
    visible,
    fade = true,
    initialInput,
    onOpened,
    onFirstOutput,
    onFailed
  }: {
    hostName: string;
    /** Laid out and on screen: fits and takes focus when this turns true. */
    visible: boolean;
    /** Dissolve scrolled output into the top edge (the full-screen tab look). */
    fade?: boolean;
    /** Typed once the shell is up (e.g. a `cd` into the folder being browsed). */
    initialInput?: string;
    /** The backend id, once the PTY is open. */
    onOpened?: (termId: number) => void;
    onFirstOutput?: () => void;
    onFailed?: (message: string) => void;
  } = $props();

  // The Nerd Font families come after the generic `monospace`, not merely after the
  // named system ones: the named list is macOS/Windows-only, so on a Linux desktop a
  // patched font ahead of the generic would become the terminal's Latin face and size
  // its cell from itself. Per-character fallback continues past a generic family, so the
  // Private Use Area glyphs (starship, powerlevel10k, eza --icons) still reach the tail.
  // Within the tail, the single-width variants come first — Nerd Fonts v3 ships icons at
  // double width in the bare family and one cell wide in its `Mono` twin.
  const MONO =
    'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace, ' +
    '"Symbols Nerd Font Mono", "Symbols Nerd Font", "MesloLGS NF", ' +
    '"JetBrainsMono Nerd Font Mono", "JetBrainsMono Nerd Font", ' +
    '"Hack Nerd Font Mono", "Hack Nerd Font", ' +
    '"FiraCode Nerd Font Mono", "FiraCode Nerd Font"';
  // One encoder for the keystroke hot path instead of one per input event.
  const ENCODER = new TextEncoder();

  // A large paste arrives as one onData; sending it as a single number[] would freeze
  // the UI thread (§9). Split into bounded chunks and await each so paint yields between
  // them; a serialization chain keeps all input strictly in order across events.
  let writeChain: Promise<void> = Promise.resolve();
  function sendInput(bytes: Uint8Array): void {
    if (termId == null || bytes.length === 0) return;
    writeChain = writeChain.then(async () => {
      for (const chunk of chunkBytes(bytes)) {
        if (destroyed || termId == null) return;
        try {
          await terminalWrite(termId, Array.from(chunk));
        } catch {
          // Stop this input on a write failure rather than sending a gapped stream.
          return;
        }
      }
    });
  }

  let container: HTMLDivElement;
  let term: Terminal | undefined;
  let fitAddon: FitAddon | undefined;
  let termId: number | undefined;
  let destroyed = false;
  let gotOutput = false;
  let ready = $state(false);
  let themeUnsub: (() => void) | undefined;
  let resizeObserver: ResizeObserver | undefined;
  let fitScheduled = false;
  // The top-edge fade dissolves scrolled output into the top edge, but never the live
  // prompt: after `clear`/Ctrl+L the cursor homes to the top, so the fade must lift
  // there (see terminalFade). Recomputed after every write too, since those resets
  // move the viewport without firing onScroll.
  let scrolled = $state(false);
  function syncScrolled(): void {
    const buf = term?.buffer.active;
    scrolled = fade && !!buf && shouldFadeTop(buf.viewportY, buf.baseY, buf.cursorY);
  }

  /** Fit the terminal to its container and tell the backend, but only while visible —
   *  a hidden (display:none) container measures 0, so it refits when shown instead. */
  function safeFit(): void {
    if (!term || !fitAddon || !visible) return;
    try {
      fitAddon.fit();
    } catch {
      return;
    }
    if (termId != null) void terminalResize(termId, term.cols, term.rows).catch(() => {});
  }

  function scheduleFit(): void {
    if (fitScheduled) return;
    fitScheduled = true;
    requestAnimationFrame(() => {
      fitScheduled = false;
      safeFit();
    });
  }

  /** Type into the shell (as if the user had), e.g. to `cd` somewhere. */
  export function type(text: string): void {
    sendInput(ENCODER.encode(text));
    term?.focus();
  }

  export function focus(): void {
    term?.focus();
  }

  onMount(() => {
    void (async () => {
      const [{ Terminal }, { FitAddon }] = await Promise.all([
        import('@xterm/xterm'),
        import('@xterm/addon-fit')
      ]);
      if (destroyed) return;

      term = new Terminal({
        fontFamily: MONO,
        fontSize: 13,
        cursorBlink: true,
        scrollback: 5000
      });
      fitAddon = new FitAddon();
      term.loadAddon(fitAddon);
      term.open(container);
      term.onScroll(syncScrolled);

      // The #1 theme-regression guard (§5.1): push the matching xterm theme to this
      // terminal — including already-open ones — whenever the store flips. Its
      // synchronous first call (pre-paint) also sets the initial theme.
      themeUnsub = theme.subscribe((t) => {
        if (term) term.options.theme = xtermTheme(t);
      });

      // Route raw output into xterm. The channel is typed `number[]`, but the raw path
      // actually delivers an `ArrayBuffer` (§3.3); `Uint8Array` wraps either.
      const channel = new Channel<TerminalBytes>();
      channel.onmessage = (msg) => {
        if (!term) return;
        if (!gotOutput) {
          gotOutput = true;
          onFirstOutput?.();
          if (initialInput) sendInput(ENCODER.encode(initialInput));
        }
        term.write(new Uint8Array(msg as unknown as ArrayBuffer), syncScrolled);
      };

      // Fit before opening so the remote PTY starts at the visible size.
      safeFit();
      const id = await terminalOpen(hostName, term.cols || 80, term.rows || 24, channel);
      if (destroyed) {
        void terminalClose(id).catch(() => {});
        return;
      }
      termId = id;
      onOpened?.(id);

      // Text keystrokes/paste are UTF-8; onBinary carries raw 8-bit sequences
      // (e.g. legacy mouse reporting) that must go byte-for-byte, not re-encoded.
      term.onData((data) => sendInput(ENCODER.encode(data)));
      term.onBinary((data) => sendInput(Uint8Array.from(data, (ch) => ch.charCodeAt(0) & 0xff)));

      resizeObserver = new ResizeObserver(() => scheduleFit());
      resizeObserver.observe(container);

      ready = true;
      if (visible) term.focus();
    })().catch((err) => {
      // `terminal_open` itself failed (e.g. the session could not be spawned): no
      // PtyExited follows, so report it here instead of leaving the terminal hung.
      onFailed?.(err instanceof Error ? err.message : String(err));
    });
  });

  onDestroy(() => {
    destroyed = true;
    themeUnsub?.();
    resizeObserver?.disconnect();
    // Idempotent: a remote-exit teardown already dropped this id backend-side (§3.4).
    if (termId != null) void terminalClose(termId).catch(() => {});
    term?.dispose();
    // Null it so a byte still in flight (destroyed-before-open race) can't write to
    // a disposed terminal — the channel callback's `if (!term)` guard then bails.
    term = undefined;
  });

  // Becoming visible: a hidden container measured 0, so refit and take focus.
  $effect(() => {
    if (visible && ready) {
      requestAnimationFrame(() => {
        safeFit();
        term?.focus();
        syncScrolled();
      });
    }
  });
</script>

<div bind:this={container} class="h-full w-full" class:term-fade={scrolled}></div>

<style>
  /* Scrolled output dissolves into the top edge instead of hard-clipping (on only while
     scrolled, so the first line stays crisp). black/transparent are mask alphas. */
  .term-fade {
    -webkit-mask-image: linear-gradient(to bottom, transparent, black 2.25rem);
    mask-image: linear-gradient(to bottom, transparent, black 2.25rem);
  }
</style>
