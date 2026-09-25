import { beforeEach, describe, expect, it } from 'vitest';
import { get } from 'svelte/store';
import type { KeySetupStepDto } from '$lib/bindings';
import {
  beginKeySetup,
  dismissKeySetup,
  keySetup,
  reduceComplete,
  reduceFailed,
  reduceProgress,
  reduceRollback,
  type KeySetupRun
} from './keySetup';

function step(index: number, description = 'Working'): KeySetupStepDto {
  return { id: 'step', index, total: 6, description };
}

describe('key-setup run lifecycle', () => {
  beforeEach(() => dismissKeySetup());

  it('beginKeySetup opens a running panel with no step yet', () => {
    beginKeySetup('web-1');
    expect(get(keySetup)).toEqual({ hostName: 'web-1', phase: { kind: 'running', step: null } });
  });

  it('dismiss clears the panel', () => {
    beginKeySetup('web-1');
    dismissKeySetup();
    expect(get(keySetup)).toBeNull();
  });
});

describe('reduceProgress', () => {
  const running: KeySetupRun = { hostName: 'web-1', phase: { kind: 'running', step: null } };

  it('advances the active host run to the reported step', () => {
    const s = step(3, 'Verifying key authentication');
    expect(reduceProgress(running, 'web-1', s)).toEqual({
      hostName: 'web-1',
      phase: { kind: 'running', step: s }
    });
  });

  it('ignores progress for a different host (single active run)', () => {
    // A stray step for another host must not hijack the active panel.
    expect(reduceProgress(running, 'db-9', step(2))).toBe(running);
  });

  it('ignores progress when no run is active', () => {
    expect(reduceProgress(null, 'web-1', step(1))).toBeNull();
  });

  it('overwrites a terminal phase only for its own host', () => {
    const done: KeySetupRun = { hostName: 'web-1', phase: { kind: 'complete', keyPath: '/k' } };
    // A late progress for the same host would re-open running (harmless); a different
    // host's progress is ignored so the completed panel stays put.
    expect(reduceProgress(done, 'db-9', step(4))).toBe(done);
  });
});

describe('terminal reducers', () => {
  it('reduceComplete carries the key path', () => {
    expect(reduceComplete('web-1', '/home/me/.ssh/omnyssh_web-1_ed25519')).toEqual({
      hostName: 'web-1',
      phase: { kind: 'complete', keyPath: '/home/me/.ssh/omnyssh_web-1_ed25519' }
    });
  });

  it('reduceFailed carries the error', () => {
    expect(reduceFailed('web-1', 'Connection failed')).toEqual({
      hostName: 'web-1',
      phase: { kind: 'failed', error: 'Connection failed' }
    });
  });

  it('reduceRollback carries the rollback result', () => {
    expect(reduceRollback('web-1', 'Restored password auth.')).toEqual({
      hostName: 'web-1',
      phase: { kind: 'rolledBack', result: 'Restored password auth.' }
    });
  });
});
