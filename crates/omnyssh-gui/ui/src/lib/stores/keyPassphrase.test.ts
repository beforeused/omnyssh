import { describe, expect, it } from 'vitest';
import { get } from 'svelte/store';
import { keyPassphrasePrompt } from './keyPassphrase';

describe('key passphrase prompt', () => {
  it('resolves only after the dialog answers', async () => {
    const answer = keyPassphrasePrompt.request({ hostName: 'work', keyPath: '/keys/work' });
    expect(get(keyPassphrasePrompt)).toEqual({ hostName: 'work', keyPath: '/keys/work' });

    keyPassphrasePrompt.answer(true);
    await expect(answer).resolves.toBe(true);
    expect(get(keyPassphrasePrompt)).toBeNull();
  });

  it('cancels an older waiter when a newer connection asks', async () => {
    const old = keyPassphrasePrompt.request({ hostName: 'old', keyPath: '/keys/old' });
    const current = keyPassphrasePrompt.request({ hostName: 'new', keyPath: '/keys/new' });

    await expect(old).resolves.toBe(false);
    keyPassphrasePrompt.answer(false);
    await expect(current).resolves.toBe(false);
  });
});
