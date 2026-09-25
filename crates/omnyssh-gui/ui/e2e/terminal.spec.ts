import { expect, test, type Page } from '@playwright/test';

// Terminal streaming vertical (tech-gui.md §3.1). e2e runs against the static SPA with
// Tauri absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). The stub is
// channel-aware: `terminal_open` captures the per-session output Channel and streams a
// prompt through it (proving raw output renders); `terminal_write` echoes a canned line
// on Enter (proving input round-trips). The host-first path (a Dashboard card's `sh`,
// no picker) is the load-bearing flow the stage requires.
const HOSTS = [
  { name: 'web-1', hostname: 'web-1.example.com', user: 'deploy', port: 22, tags: ['prod'], source: 'manual', hasKey: true },
  { name: 'db-1', hostname: 'db-1.example.com', user: 'root', port: 22, tags: [], source: 'manual', hasKey: false }
];

async function boot(page: Page): Promise<void> {
  await page.addInitScript(
    ({ hosts }) => {
      let cbid = 0;
      const win = window as unknown as Record<string, unknown>;
      const listeners: Record<string, number[]> = {};
      // Per-channel outgoing index — the real Channel enforces message ordering.
      const chIndex: Record<number, number> = {};
      // Backend session id -> its output channel id, so terminal_write can echo.
      const sessionChannel: Record<number, number> = {};
      let nextSession = 0;

      function sendToChannel(chId: number, text: string): void {
        const cb = win[`__cb${chId}`] as ((m: unknown) => void) | undefined;
        const index = chIndex[chId] ?? 0;
        chIndex[chId] = index + 1;
        // The raw path delivers an ArrayBuffer; mirror that so xterm's Uint8Array wrap works.
        cb?.({ message: new TextEncoder().encode(text).buffer, index });
      }

      function fireEvent(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          const cb = win[`__cb${id}`] as ((e: unknown) => void) | undefined;
          cb?.({ event, id, payload });
        }
      }
      // Lets a test simulate the remote shell exiting for a given backend session id.
      (win as { __fireTerminalExited?: (sessionId: number) => void }).__fireTerminalExited = (
        sessionId
      ) => fireEvent('terminal-exited', { sessionId });

      (win as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve(hosts);
            case 'reload_hosts':
              return Promise.resolve(null);
            case 'terminal_open': {
              const chId = (args.onOutput as { id: number }).id;
              const sid = ++nextSession;
              sessionChannel[sid] = chId;
              // A shell prompt proves the streamed output renders + flips status to connected.
              setTimeout(() => sendToChannel(chId, 'omnyssh-ready> '), 0);
              return Promise.resolve(sid);
            }
            case 'terminal_write': {
              const { sessionId, data } = args as { sessionId: number; data: number[] };
              const chId = sessionChannel[sessionId];
              // Echo a canned result once Enter (\r == 13) arrives, so output is assertable.
              if (chId != null && data.includes(13)) {
                setTimeout(() => sendToChannel(chId, '\r\nRESULT-OK\r\n'), 0);
              }
              return Promise.resolve(null);
            }
            case 'terminal_resize':
            case 'terminal_close':
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
  // The status-bar total confirms the app booted and `list_hosts` resolved.
  await expect(page.getByText('2 hosts')).toBeVisible();
}

test('host-first: spawn a terminal from a card, run a command, see output, then close', async ({
  page
}) => {
  await boot(page);

  // Host-first spawn — a Dashboard card's `sh`, no picker (tech-gui.md §2, §3.1).
  await page.getByTitle('sh on web-1').click();

  // The tab row appears and the terminal renders the streamed prompt.
  await expect(page.getByRole('button', { name: 'web-1 · terminal', exact: true })).toBeVisible();
  await expect(page.locator('.xterm')).toBeVisible();
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');

  // Run a command: focus the terminal input, type, press Enter -> canned output streams back.
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.type('hi');
  await page.keyboard.press('Enter');
  await expect(page.locator('.xterm-rows')).toContainText('RESULT-OK');

  // Closing the tab tears the terminal down.
  await page.getByRole('button', { name: 'Close web-1', exact: true }).click();
  await expect(page.locator('.xterm')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'web-1 · terminal', exact: true })).toHaveCount(0);
});

test('action-first: the Terminal spawner opens the host picker, then a live terminal', async ({
  page
}) => {
  await boot(page);

  // Action-first spawn — the sidebar Terminal spawner opens the host picker (§2).
  await page.getByRole('button', { name: 'Terminal', exact: true }).click();
  await page.getByRole('dialog').getByText('web-1', { exact: true }).click();

  await expect(page.getByRole('button', { name: 'web-1 · terminal', exact: true })).toBeVisible();
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');
});

test('toggling the theme re-themes a live terminal (§5.1)', async ({ page }) => {
  await boot(page);
  await page.getByTitle('sh on web-1').click();
  await expect(page.locator('.xterm-rows')).toContainText('omnyssh-ready');

  // xterm paints its scrollable viewport inline with theme.background; find that
  // element's computed colour rather than assume a class (robust across versions).
  const paintedBg = () =>
    page.evaluate(() => {
      const root = document.querySelector('.xterm');
      const els = root ? Array.from(root.querySelectorAll<HTMLElement>('*')) : [];
      const painted = els.find((el) => el.style.backgroundColor);
      return painted ? getComputedStyle(painted).backgroundColor : '';
    });

  // App defaults to dark → the dark surface (#212121).
  await expect.poll(paintedBg).toBe('rgb(33, 33, 33)');

  // The #1 theme-regression guard: flipping the store re-themes the OPEN terminal.
  await page.getByTitle('Switch to light theme').click();
  await expect.poll(paintedBg).toBe('rgb(255, 255, 255)');
});

test('a remote exit (terminal-exited) tears the tab down', async ({ page }) => {
  await boot(page);
  await page.getByTitle('sh on web-1').click();
  await expect(page.getByRole('button', { name: 'web-1 · terminal', exact: true })).toBeVisible();
  await expect(page.locator('.xterm')).toBeVisible();

  // The remote shell exits: the backend emits terminal-exited for session id 1.
  await page.evaluate(() => {
    (window as unknown as { __fireTerminalExited: (id: number) => void }).__fireTerminalExited(1);
  });

  await expect(page.getByRole('button', { name: 'web-1 · terminal', exact: true })).toHaveCount(0);
  await expect(page.locator('.xterm')).toHaveCount(0);
});
