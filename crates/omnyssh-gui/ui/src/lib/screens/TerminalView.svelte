<script lang="ts">
  // A live terminal tab (tech-gui.md §3.1). One instance per terminal session, kept
  // mounted for the session's whole life — hidden, not destroyed, when another entity
  // is active — so scrollback and the byte stream survive tab switches. The xterm and
  // PTY plumbing lives in TerminalPane; this maps its lifecycle onto the tab (sidebar
  // status dot, backend id, remote-exit teardown).
  import TerminalPane from './TerminalPane.svelte';
  import { sessions, type Session } from '$lib/stores/sessions';
  import { closeSession } from '$lib/stores/navigation';
  import { terminalDidExit } from '$lib/ipc/router';
  import { lastError } from '$lib/stores/notifications';

  let { session, active }: { session: Session; active: boolean } = $props();

  function opened(id: number): void {
    sessions.setTermId(session.id, id);
    // The remote may have already exited before this id was recorded (fast-fail
    // connect race): terminal-exited couldn't match the tab, so close it now.
    if (terminalDidExit(id)) closeSession(session.id);
  }

  function failed(message: string): void {
    lastError.set(message);
    sessions.setStatus(session.id, 'failed');
  }
</script>

<!-- bg-surface fills behind the macOS traffic lights (no seam). Text selection stays
     disabled app-wide (app.css); the terminal is the one selectable surface, handled
     by xterm's own selection (not CSS). -->
<div class="absolute inset-0 overflow-hidden bg-surface {active ? '' : 'hidden'}">
  <!-- Inset via this wrapper, not the xterm host: padding on the element xterm mounts
       into makes FitAddon over-size, sliding the last row under the status bar. The top
       inset clears the macOS traffic-light strip; the bottom gap clears the footer. -->
  <div class="h-full w-full" style="padding: max(var(--titlebar-h), 0.75rem) 0.5rem 1rem;">
    <TerminalPane
      hostName={session.hostName}
      visible={active}
      onOpened={opened}
      onFirstOutput={() => sessions.setStatus(session.id, 'connected')}
      onFailed={failed}
    />
  </div>
</div>
