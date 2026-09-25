import { expect, test, type Page } from '@playwright/test';

// Auto SSH-key setup (tech-gui.md §4.2). e2e runs against the static SPA with Tauri
// absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). `start_key_setup` is
// faked: it streams `key-setup-progress` events and then `key-setup-complete`, flipping
// the host's hasKey/passwordAuthDisabled so the follow-up `reload_hosts` replays a keyed
// host — exactly how the real backend drives the panel and refreshes the card.
const HOSTS = [
  { name: 'pw-host', hostname: 'pw.example.com', user: 'root', port: 22, tags: [], source: 'manual', hasKey: false }
];

async function boot(page: Page): Promise<void> {
  await page.addInitScript(
    ({ hosts }) => {
      let cbid = 0;
      const listeners: Record<string, number[]> = {};
      const state: { hosts: Array<Record<string, unknown>> } = { hosts: hosts.map((h) => ({ ...h })) };
      const win = window as unknown as Record<string, unknown>;

      function fire(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          const cb = win[`__cb${id}`] as ((e: unknown) => void) | undefined;
          cb?.({ event, id, payload });
        }
      }

      (win as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve([...state.hosts]);
            case 'reload_hosts':
              setTimeout(() => fire('hosts-loaded', [...state.hosts]), 0);
              return Promise.resolve(null);
            case 'list_ssh_keys':
              return Promise.resolve([
                { path: '/home/me/.ssh/id_ed25519', name: 'id_ed25519', kind: 'ed25519', comment: 'me@laptop', encrypted: false },
                { path: '/home/me/.ssh/work', name: 'work', kind: 'rsa', encrypted: false }
              ]);
            case 'get_default_key':
              return Promise.resolve('/home/me/.ssh/work');
            case 'host_auth':
              return Promise.resolve({ hasPassword: true });
            case 'start_key_setup': {
              const name = args.hostName as string;
              (win as { __keySetupArgs?: unknown }).__keySetupArgs = { key: args.key, mode: args.mode };
              const keyOnly = args.mode === 'keyOnly';
              const key = args.key as { kind: string; path?: string };
              const keyPath = key.kind === 'existing' ? key.path : `/home/me/.ssh/${(key as { name?: string }).name ?? `omnyssh_${name}_ed25519`}`;
              const step = (id: string, index: number) =>
                fire('key-setup-progress', { hostName: name, step: { id, index, total: 6, description: id } });
              setTimeout(() => step('generateKey', 1), 40);
              setTimeout(() => step('verifyKeyAuth', 3), 120);
              setTimeout(() => {
                // The real backend persists the key before emitting complete; mirror
                // that so the panel's reload shows a keyed host.
                const h = state.hosts.find((x) => (x as { name: string }).name === name);
                if (h) {
                  h.hasKey = true;
                  h.passwordAuthDisabled = keyOnly;
                }
                fire('key-setup-complete', {
                  hostName: name,
                  keyPath,
                  passwordAuthDisabled: keyOnly,
                  partial: false
                });
              }, 400);
              return Promise.resolve(null);
            }
            case 'plugin:event|listen': {
              const { event, handler } = args as { event: string; handler: number };
              (listeners[event] ||= []).push(handler);
              return Promise.resolve(cbid);
            }
            default:
              return Promise.resolve(null);
          }
        },
        transformCallback: (cb: unknown) => {
          const id = ++cbid;
          win[`__cb${id}`] = cb;
          return id;
        }
      };
    },
    { hosts: HOSTS }
  );
  await page.goto('/');
  await expect(page.getByText('pw-host', { exact: true })).toBeVisible();
}

test('the key button installs an existing key, key-only, and the card reflects it', async ({ page }) => {
  await boot(page);

  // Every card carries the key button.
  await page.getByRole('button', { name: 'SSH key for pw-host' }).click();
  const setup = page.getByRole('dialog', { name: 'SSH key — pw-host' });
  await expect(setup).toBeVisible();

  // With no key of its own, the host starts on the default key from Settings; the
  // found keys are listed as cards.
  await expect(setup.getByRole('tab', { name: 'Existing key' })).toHaveAttribute('aria-selected', 'true');
  await expect(setup.getByRole('radio', { name: 'work', exact: true })).toHaveAttribute('aria-checked', 'true');
  // Pick another found key, and key-only logins.
  await setup.getByRole('radio', { name: 'id_ed25519', exact: true }).click();
  await expect(setup.getByRole('radio', { name: 'id_ed25519', exact: true })).toHaveAttribute('aria-checked', 'true');
  await setup.getByRole('radio', { name: 'Key only' }).check();
  await setup.getByRole('button', { name: 'Install key' }).click();

  const progress = page.getByRole('dialog', { name: 'SSH key setup — pw-host' });
  await expect(progress).toBeVisible();
  await expect(progress.getByText('Checking that the key logs in')).toBeVisible();
  await expect(progress.getByText('Key installed')).toBeVisible();
  await expect(progress.getByText('Only key logins are allowed now.')).toBeVisible();
  await expect(progress.getByText('/home/me/.ssh/id_ed25519')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __keySetupArgs: unknown }).__keySetupArgs)).toEqual({
    key: { kind: 'existing', path: '/home/me/.ssh/id_ed25519' },
    mode: 'keyOnly'
  });

  await progress.getByRole('button', { name: 'Done' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByText('key-only')).toBeVisible();
  // The button stays: the key and login mode can be changed any time.
  await expect(page.getByRole('button', { name: 'SSH key for pw-host' })).toBeVisible();
});

test('a new key with password and key logins keeps passwords on', async ({ page }) => {
  await boot(page);
  await page.getByRole('button', { name: 'SSH key for pw-host' }).click();
  const setup = page.getByRole('dialog', { name: 'SSH key — pw-host' });
  await setup.getByRole('tab', { name: 'New key' }).click();
  const name = setup.getByRole('textbox');
  await expect(name).toHaveValue('omnyssh_pw-host_ed25519');
  // A name already in ~/.ssh, or one with a path in it, is refused.
  await name.fill('work');
  await expect(setup.getByText(/already exists/)).toBeVisible();
  await expect(setup.getByRole('button', { name: 'Install key' })).toBeDisabled();
  await name.fill('../evil');
  await expect(setup.getByText(/Use only Latin letters/)).toBeVisible();
  await name.fill('deploy_prod');
  await expect(setup.getByText('An Ed25519 key. Saved as ~/.ssh/deploy_prod and ~/.ssh/deploy_prod.pub.')).toBeVisible();
  await expect(setup.getByRole('radio', { name: 'Password and key' })).toBeChecked();
  await setup.getByRole('button', { name: 'Install key' }).click();

  const progress = page.getByRole('dialog', { name: 'SSH key setup — pw-host' });
  await expect(progress.getByText('Both the key and the password log in.')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __keySetupArgs: unknown }).__keySetupArgs)).toEqual({
    key: { kind: 'generate', name: 'deploy_prod' },
    mode: 'keyAndPassword'
  });
  await progress.getByRole('button', { name: 'Done' }).click();
  await expect(page.getByText('key', { exact: true })).toBeVisible();
});
