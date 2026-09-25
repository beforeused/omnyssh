import { expect, test, type Page } from '@playwright/test';

// Settings + self-update (tech-gui.md §4.3). e2e runs against the static SPA with Tauri
// absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). The stub backs the
// update config in memory and returns an update from `check_update`; `update-available`
// is fired after `reload_hosts` (which the layout calls once its listeners are attached),
// mirroring the startup check.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: [], source: 'manual', hasKey: true }
];

const UPDATE = {
  version: '2.0.0',
  url: 'https://github.com/timhartmann7/omnyssh/releases/tag/v2.0.0',
  tag: 'v2.0.0',
  canSelfUpdate: true
};

async function boot(page: Page, opts: { fireUpdateOnBoot: boolean }): Promise<void> {
  await page.addInitScript(
    ({ hosts, update, fireUpdateOnBoot }) => {
      let cbid = 0;
      const listeners: Record<string, number[]> = {};
      const state = {
        hosts: hosts.map((h) => ({ ...h })),
        updateConfig: { checkOnStartup: true, skipVersion: '' } as Record<string, unknown>
      };
      const win = window as unknown as Record<string, unknown>;

      function fire(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          const cb = win[`__cb${id}`] as ((e: unknown) => void) | undefined;
          cb?.({ event, id, payload });
        }
      }

      (win as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          if (cmd.startsWith('plugin:opener')) {
            ((win as { __opened?: unknown[] }).__opened ||= []).push(args.url);
            return Promise.resolve(null);
          }
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve([...state.hosts]);
            case 'reload_hosts':
              setTimeout(() => {
                fire('hosts-loaded', [...state.hosts]);
                if (fireUpdateOnBoot) fire('update-available', { info: update });
              }, 0);
              return Promise.resolve(null);
            case 'refresh_metrics':
              return Promise.resolve(null);
            case 'load_update_config':
              return Promise.resolve({ ...state.updateConfig });
            case 'save_update_config':
              state.updateConfig = { ...(args.config as Record<string, unknown>) };
              win.__savedUpdateConfig = { ...(args.config as Record<string, unknown>) };
              return Promise.resolve(null);
            case 'check_update':
              return Promise.resolve({ ...update });
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
    { hosts: HOSTS, update: UPDATE, fireUpdateOnBoot: opts.fireUpdateOnBoot }
  );
  await page.goto('/');
  await expect(page.getByText('web-1', { exact: true })).toBeVisible();
}

test('the footer gear opens Settings; theme and interval work, and About credits the fork', async ({
  page
}) => {
  await boot(page, { fireUpdateOnBoot: false });

  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();

  // Theme mirrors the sidebar toggle (§5.1): the app boots dark; picking Light flips it.
  // `exact` avoids the sidebar toggle whose label reads "Switch to light theme".
  await page.getByRole('button', { name: 'Light', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await page.getByRole('button', { name: 'Dark', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

  // Auto-refresh interval is a segmented pref.
  const tenSec = page.getByRole('button', { name: '10s', exact: true });
  await tenSec.click();
  await expect(tenSec).toHaveAttribute('aria-pressed', 'true');

  // No upstream update controls in this fork; About names the developer instead.
  await expect(page.getByRole('switch', { name: 'Check for updates on startup' })).toHaveCount(0);
  await expect(page.getByText('This build is a fork of OmnySSH')).toBeVisible();
  await expect(page.getByText('beforeused', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: '@beforeused' }).click();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __opened?: unknown[] }).__opened))
    .toEqual(['https://t.me/beforeused']);
});

test('startup update-available raises the banner; dismiss hides it', async ({ page }) => {
  await boot(page, { fireUpdateOnBoot: true });

  const banner = page.getByText('Update available — v2.0.0');
  await expect(banner).toBeVisible();

  await page.getByRole('button', { name: 'Dismiss update notice' }).click();
  await expect(banner).toHaveCount(0);
});

test('switching the language to Russian translates the interface and persists', async ({ page }) => {
  await boot(page, { fireUpdateOnBoot: false });
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByRole('button', { name: 'Русский' }).click();
  await expect(page.getByRole('heading', { name: 'Настройки' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Настройки' })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('lang', 'ru');
  await page.reload();
  await expect(page.getByRole('button', { name: 'Настройки' })).toBeVisible();
  await page.getByRole('button', { name: 'Настройки' }).click();
  await page.getByRole('button', { name: 'English' }).click();
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
});
