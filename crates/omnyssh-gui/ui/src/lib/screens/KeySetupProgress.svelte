<script lang="ts">
  // SSH key setup progress panel (tech-gui.md §4.2). Renders the active run from the
  // keySetup store: a stepped progress bar while running, then the outcome (success /
  // failure / rollback). On the first 'complete' it reloads the host list once — the
  // backend already wrote hosts.toml, so the card must refresh to reflect the new key
  // and login mode. Mounted globally (AppShell) so it survives navigating away from the
  // Dashboard mid-run. Step labels come from the step id, in the UI language.
  import Modal from '$lib/components/Modal.svelte';
  import { Button, Icon, StatusDot } from '$lib/theme';
  import { keySetup, dismissKeySetup } from '$lib/stores/keySetup';
  import { reloadHosts } from '$lib/ipc/commands';
  import { lastError } from '$lib/stores/notifications';
  import { t, type MessageKey } from '$lib/i18n';

  const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));

  const STEP_KEYS: Record<string, MessageKey> = {
    generateKey: 'keysetup.step.generateKey',
    copyPublicKey: 'keysetup.step.copyPublicKey',
    verifyKeyAuth: 'keysetup.step.verifyKeyAuth',
    disablePassword: 'keysetup.step.disablePassword',
    enablePassword: 'keysetup.step.enablePassword',
    reloadSshd: 'keysetup.step.reloadSshd',
    finalCheck: 'keysetup.step.finalCheck'
  };

  // Reload hosts once per completed run. Cleared while a run is not complete, so
  // re-running setup on the same host reloads again.
  let reloadedFor = $state<string | null>(null);
  $effect(() => {
    const run = $keySetup;
    if (!run) return;
    if (run.phase.kind === 'complete') {
      if (reloadedFor !== run.hostName) {
        reloadedFor = run.hostName;
        reloadHosts().catch((e) => lastError.set(message(e)));
      }
    } else if (reloadedFor === run.hostName) {
      reloadedFor = null;
    }
  });
</script>

{#if $keySetup}
  {@const run = $keySetup}
  {@const phase = run.phase}
  <Modal label={$t('keysetup.progressTitle', { host: run.hostName })} onClose={dismissKeySetup}>
    <div class="space-y-4 px-5 py-4">
      <div class="flex items-center gap-2.5">
        <Icon name="key" size={16} />
        <h2 class="min-w-0 truncate text-sm font-semibold">
          {$t('keysetup.progressTitle', { host: run.hostName })}
        </h2>
      </div>

      {#if phase.kind === 'running'}
        {@const index = phase.step?.index ?? 0}
        {@const total = phase.step?.total ?? 6}
        {@const stepKey = phase.step ? STEP_KEYS[phase.step.id] : undefined}
        <div class="space-y-2">
          <div class="flex items-center justify-between gap-3 text-xs">
            <span class="min-w-0 truncate text-muted">
              {stepKey ? $t(stepKey) : (phase.step?.description ?? $t('keysetup.connecting'))}
            </span>
            <span class="shrink-0 tabular-nums text-faint">{index}/{total}</span>
          </div>
          <div class="h-1.5 overflow-hidden rounded-full bg-surface-inset">
            <div
              class="h-full rounded-full bg-accent transition-[width] duration-300"
              style="width: {total ? Math.round((index / total) * 100) : 0}%"
            ></div>
          </div>
        </div>
        <p class="text-xs text-faint">{$t('keysetup.runningNote')}</p>
      {:else if phase.kind === 'complete'}
        <div class="flex items-start gap-2.5">
          <span class="mt-0.5 shrink-0">
            <StatusDot status={phase.partial ? 'warn' : 'ok'} size={9} />
          </span>
          <div class="min-w-0 space-y-1">
            <p class="text-sm font-medium">{$t('keysetup.done')}</p>
            <p class="break-all font-mono text-xs text-muted">{phase.keyPath}</p>
            <p class="text-xs text-muted">
              {#if phase.partial}
                {$t('keysetup.partial')}
              {:else if phase.passwordAuthDisabled === true}
                {$t('keysetup.doneKeyOnly')}
              {:else if phase.passwordAuthDisabled === false}
                {$t('keysetup.doneBoth')}
              {:else}
                {$t('keysetup.doneUnknown')}
              {/if}
            </p>
          </div>
        </div>
        <div class="flex justify-end">
          <Button variant="primary" onclick={dismissKeySetup}>{$t('common.done')}</Button>
        </div>
      {:else if phase.kind === 'failed'}
        <div class="flex items-start gap-2.5">
          <span class="mt-0.5 shrink-0"><StatusDot status="crit" size={9} /></span>
          <div class="min-w-0 space-y-1">
            <p class="text-sm font-medium">{$t('keysetup.failed')}</p>
            <p class="break-words text-xs text-muted">{phase.error}</p>
            <p class="text-xs text-faint">{$t('keysetup.failedNote')}</p>
          </div>
        </div>
        <div class="flex justify-end">
          <Button variant="ghost" onclick={dismissKeySetup}>{$t('common.close')}</Button>
        </div>
      {:else}
        <div class="flex items-start gap-2.5">
          <span class="mt-0.5 shrink-0"><StatusDot status="warn" size={9} /></span>
          <div class="min-w-0 space-y-1">
            <p class="text-sm font-medium">{$t('keysetup.rolledBack')}</p>
            <p class="break-words text-xs text-muted">{phase.result}</p>
            <p class="text-xs text-faint">{$t('keysetup.rolledBackNote')}</p>
          </div>
        </div>
        <div class="flex justify-end">
          <Button variant="ghost" onclick={dismissKeySetup}>{$t('common.close')}</Button>
        </div>
      {/if}
    </div>
  </Modal>
{/if}
