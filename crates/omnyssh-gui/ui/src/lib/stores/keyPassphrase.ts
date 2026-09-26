import { writable } from 'svelte/store';

export interface KeyPassphraseRequest {
  hostName: string;
  keyPath: string;
}

/** One app-wide passphrase prompt. A connection waits for its answer before the
 * session tab is created, so a cancelled prompt never leaves a dead tab behind. */
function createKeyPassphrasePrompt() {
  const { subscribe, set } = writable<KeyPassphraseRequest | null>(null);
  let finish: ((proceed: boolean) => void) | undefined;

  return {
    subscribe,
    request(request: KeyPassphraseRequest): Promise<boolean> {
      finish?.(false);
      set(request);
      return new Promise<boolean>((resolve) => {
        finish = resolve;
      });
    },
    answer(proceed: boolean): void {
      const resolve = finish;
      finish = undefined;
      set(null);
      resolve?.(proceed);
    }
  };
}

export const keyPassphrasePrompt = createKeyPassphrasePrompt();
