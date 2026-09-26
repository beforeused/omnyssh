import { expect, test, type Page } from '@playwright/test';

// The Docker panel. Tauri is stubbed at `__TAURI_INTERNALS__`: the host's discovery
// reports Docker (so the card shows the `docker` button), `docker_list` serves two
// containers, and actions / logs / the shell command are recorded.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: [], source: 'manual', hasKey: true, monitoring: 'ssh' }
];

async function boot(page: Page): Promise<void> {
  await page.addInitScript(
    ({ hosts }) => {
      let cbid = 0;
      const win = window as unknown as Record<string, unknown>;
      const listeners: Record<string, number[]> = {};
      const calls: Array<{ cmd: string; args: unknown }> = [];
      win.__calls = calls;
      const containers = [
        { id: 'aaa', name: 'web', image: 'nginx:1.27', state: 'running', status: 'Up 3 hours', ports: '0.0.0.0:80->80/tcp', created: '', cpu: '0.5%', memory: '20MiB / 1GiB' },
        { id: 'bbb', name: 'db', image: 'postgres:16', state: 'exited', status: 'Exited (0) 2 days ago', ports: '', created: '' }
      ];
      function fire(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          (win[`__cb${id}`] as ((e: unknown) => void) | undefined)?.({ event, id, payload });
        }
      }
      win.__fire = fire;
      win.__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          calls.push({ cmd, args });
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve(hosts);
            case 'reload_hosts':
              setTimeout(() => fire('hosts-loaded', hosts), 0);
              return Promise.resolve(null);
            case 'docker_list':
              return Promise.resolve({ containers, sudo: false });
            case 'docker_logs':
              return Promise.resolve('2026-09-25T10:00:00Z listening on :80\n');
            case 'docker_shell_command':
              return Promise.resolve(`docker exec -it '${args.id}' sh`);
            case 'docker_action':
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
    { hosts: HOSTS }
  );
  await page.goto('/');
  await expect(page.getByText('web-1', { exact: true })).toBeVisible();
  // Discovery finds Docker (fired once the app is listening).
  await page.evaluate(() =>
    (window as unknown as { __fire: (e: string, p: unknown) => void }).__fire('services-detected', {
      hostName: 'web-1',
      services: [{ kind: 'docker', metrics: [] }]
    })
  );
}

const calls = (page: Page) =>
  page.evaluate(() => (window as unknown as { __calls: Array<{ cmd: string; args: unknown }> }).__calls);

test('the card opens a Docker tab with containers, actions, logs and a shell', async ({ page }) => {
  await boot(page);
  await page.getByRole('button', { name: 'docker', exact: true }).click();

  await expect(page.getByRole('heading', { name: 'Docker — web-1' })).toBeVisible();
  await expect(page.getByText('nginx:1.27')).toBeVisible();
  await expect(page.getByText('Exited (0) 2 days ago')).toBeVisible();
  await expect(page.getByText('20MiB / 1GiB')).toBeVisible();

  // Restart the running one; start the stopped one.
  await page.getByRole('button', { name: 'Restart web' }).click();
  await page.getByRole('button', { name: 'Start db' }).click();
  await expect
    .poll(async () => (await calls(page)).filter((c) => c.cmd === 'docker_action').map((c) => c.args))
    .toEqual([
      { hostName: 'web-1', id: 'aaa', action: 'restart' },
      { hostName: 'web-1', id: 'bbb', action: 'start' }
    ]);

  // Remove asks first.
  await page.getByRole('button', { name: 'Remove db' }).click();
  const confirm = page.getByRole('dialog', { name: 'Remove container?' });
  await confirm.getByRole('button', { name: 'Remove' }).click();
  await expect
    .poll(async () => (await calls(page)).some((c) => c.cmd === 'docker_action' && (c.args as { action: string }).action === 'remove'))
    .toBe(true);

  // Logs.
  await page.getByRole('button', { name: 'Logs' }).first().click();
  const logs = page.getByRole('dialog', { name: 'Logs — web' });
  await expect(logs.getByText('listening on :80')).toBeVisible();
  await logs.getByRole('button', { name: 'Close' }).last().click();

  // Shell: a terminal tab named after the container.
  await page.getByRole('button', { name: 'Shell' }).click();
  await expect(page.getByRole('button', { name: 'web-1 · web · terminal', exact: true })).toBeVisible();
});
