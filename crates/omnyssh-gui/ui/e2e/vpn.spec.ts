import { expect, test, type Page } from '@playwright/test';

// Tunnelblick: the launch banner offers the install when it is missing (the stubbed
// `vpn_install` streams progress events), and the host form picks a configuration.
const HOSTS = [
  { name: 'office', hostname: '10.8.0.5', user: 'root', port: 22, tags: [], source: 'manual', hasKey: false, monitoring: 'ssh' }
];

async function boot(page: Page, installed: boolean): Promise<void> {
  await page.addInitScript(
    ({ hosts, installed }) => {
      let cbid = 0;
      const win = window as unknown as Record<string, unknown>;
      const listeners: Record<string, number[]> = {};
      const state = { installed, saved: null as unknown };
      win.__vpn = state;
      function fire(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          (win[`__cb${id}`] as ((e: unknown) => void) | undefined)?.({ event, id, payload });
        }
      }
      win.__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve(hosts);
            case 'reload_hosts':
              setTimeout(() => fire('hosts-loaded', hosts), 0);
              return Promise.resolve(null);
            case 'vpn_status':
              return Promise.resolve({ supported: true, installed: state.installed, installsVersion: '9.0.1 (build 6491)' });
            case 'vpn_configurations':
              return (win.__failConfigs as boolean | undefined)
                ? Promise.reject(new Error('Tunnelblick: execution error (-1708)'))
                : Promise.resolve(['office-vpn', 'home']);
            case 'vpn_install':
              return new Promise((resolve) => {
                fire('vpn-install-progress', { stage: 'downloading', done: 10, total: 20 });
                setTimeout(() => {
                  fire('vpn-install-progress', { stage: 'verifying', done: 0, total: 0 });
                  fire('vpn-install-progress', { stage: 'installing', done: 0, total: 0 });
                  state.installed = true;
                  fire('vpn-install-progress', { stage: 'done', done: 0, total: 0 });
                  resolve(null);
                }, 300);
              });
            case 'save_host':
              state.saved = args.input;
              return Promise.resolve(null);
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
        },
        unregisterCallback: (id: number) => {
          delete win[`__cb${id}`];
        }
      };
    },
    { hosts: HOSTS, installed }
  );
  await page.goto('/');
  await expect(page.getByText('office', { exact: true })).toBeVisible();
}

test('launch banner installs Tunnelblick when it is missing', async ({ page }) => {
  await boot(page, false);
  const banner = page.getByRole('status', { name: 'Tunnelblick' });
  await expect(banner.getByText('Tunnelblick is not installed')).toBeVisible();
  await banner.getByRole('button', { name: 'Install' }).click();
  await expect(banner.getByText(/Tunnelblick is installed/)).toBeVisible();
  await banner.getByRole('button', { name: 'Close' }).click();
  await expect(banner).toHaveCount(0);
});

test('"don’t ask again" keeps the banner away', async ({ page }) => {
  await boot(page, false);
  const banner = page.getByRole('status', { name: 'Tunnelblick' });
  await banner.getByRole('button', { name: 'Don’t ask again' }).click();
  await expect(banner).toHaveCount(0);
  await page.reload();
  await expect(page.getByText('office', { exact: true })).toBeVisible();
  await expect(page.getByRole('status', { name: 'Tunnelblick' })).toHaveCount(0);
});

test('a host picks a Tunnelblick configuration', async ({ page }) => {
  await boot(page, true);
  await expect(page.getByRole('status', { name: 'Tunnelblick' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Edit office' }).click();
  const editor = page.getByRole('dialog', { name: 'Edit host' });
  await editor.getByRole('combobox').nth(1).selectOption('office-vpn');
  await editor.getByRole('button', { name: 'Save' }).click();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __vpn: { saved: { vpn?: string } } }).__vpn.saved?.vpn))
    .toBe('office-vpn');
});

test('a failed configuration listing says why', async ({ page }) => {
  await page.addInitScript(() => {
    (window as unknown as { __failConfigs: boolean }).__failConfigs = true;
  });
  await boot(page, true);
  await page.getByRole('button', { name: 'Edit office' }).click();
  const editor = page.getByRole('dialog', { name: 'Edit host' });
  await expect(editor.getByText(/Could not read Tunnelblick configurations: .*-1708/)).toBeVisible();
});
