import { expect, test, type Page } from '@playwright/test';

// SFTP dual-pane vertical (tech-gui.md §3.2). e2e runs against the static SPA with
// Tauri absent, so we stub `__TAURI_INTERNALS__` at the boundary (§6.4). The stub owns
// an in-memory local + remote filesystem: `list_local_dir` returns directly, `sftp_*`
// commands fire the stamped `sftp-*` events the per-session forwarder would emit, and a
// transfer holds at a progress tick until `__completeTransfer()` fires its op-done — so
// the live progress bar is deterministically observable. Both spawn paths (a card's
// `files`, and the SFTP spawner via the host picker) are load-bearing for the stage.
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
      let nextSession = 0;
      let nextTransfer = 0;
      // A transfer holds at a progress tick until the test completes it, so the queue
      // panel is observable mid-flight.
      const completions: Array<() => void> = [];
      let nextBatch = 0;
      const batches: Record<number, { direction: 'upload' | 'download'; sources: string[]; destDir: string }> = {};
      const transfersLog: Array<{ id: number; direction: string; name: string }> = [];
      const opened: Array<{ cmd: string; path: string }> = [];
      (win as { __opened?: unknown }).__opened = opened;
      (win as { __fire?: unknown }).__fire = (event: string, payload: unknown) => fireEvent(event, payload);

      type Entry = { name: string; path: string; size: number; isDir: boolean };
      const local: Record<string, Entry[]> = {
        '/home/user': [
          { name: 'notes.txt', path: '/home/user/notes.txt', size: 24, isDir: false },
          { name: 'work', path: '/home/user/work', size: 0, isDir: true }
        ]
      };
      const remote: Record<string, Entry[]> = {
        '/': [
          { name: 'config.yml', path: '/config.yml', size: 64, isDir: false },
          { name: 'var', path: '/var', size: 0, isDir: true }
        ]
      };

      function parentOf(p: string): string {
        const i = p.lastIndexOf('/');
        return i <= 0 ? '/' : p.slice(0, i);
      }
      function baseName(p: string): string {
        return p.slice(p.lastIndexOf('/') + 1);
      }
      function withParent(path: string, entries: Entry[]): Entry[] {
        if (path === '/') return entries;
        return [{ name: '..', path: parentOf(path), size: 0, isDir: true }, ...entries];
      }
      function addFile(fs: Record<string, Entry[]>, dir: string, name: string): void {
        const list = (fs[dir] ||= []);
        if (!list.some((e) => e.name === name)) {
          list.push({ name, path: dir === '/' ? `/${name}` : `${dir}/${name}`, size: 8, isDir: false });
        }
      }

      function fireEvent(event: string, payload: unknown): void {
        for (const id of listeners[event] ?? []) {
          const cb = win[`__cb${id}`] as ((e: unknown) => void) | undefined;
          cb?.({ event, id, payload });
        }
      }

      // Finish the oldest still-running transfer (deterministic completion).
      (win as { __completeTransfer?: () => void }).__completeTransfer = () => completions.shift()?.();

      (win as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        invoke: (cmd: string, args: Record<string, unknown>) => {
          if (cmd.startsWith('plugin:path')) return Promise.resolve('/home/user');
          switch (cmd) {
            case 'list_hosts':
              return Promise.resolve(hosts);
            case 'reload_hosts':
              return Promise.resolve(null);
            case 'list_local_dir': {
              const path = args.path as string;
              return Promise.resolve(withParent(path, local[path] ?? []));
            }
            case 'sftp_open': {
              const sid = ++nextSession;
              setTimeout(() => fireEvent('sftp-connected', { sessionId: sid, hostName: args.hostName }), 0);
              return Promise.resolve(sid);
            }
            case 'sftp_list': {
              const { sessionId, path } = args as { sessionId: number; path: string };
              setTimeout(
                () => fireEvent('sftp-dir-listed', { sessionId, path, entries: withParent(path, remote[path] ?? []) }),
                0
              );
              return Promise.resolve(null);
            }
            case 'transfer_prepare': {
              // Walk nothing (flat files only) and flag names that already exist.
              const { direction, sources, destDir } = args as {
                direction: 'upload' | 'download';
                sources: string[];
                destDir: string;
              };
              const target = direction === 'upload' ? remote : local;
              const batchId = ++nextBatch;
              batches[batchId] = { direction, sources, destDir };
              const conflicts = sources
                .map((src, index) => ({ src, index }))
                .filter(({ src }) => (target[destDir] ?? []).some((e) => e.name === baseName(src)))
                .map(({ src, index }) => {
                  const name = baseName(src);
                  const dot = name.lastIndexOf('.');
                  const altName = dot > 0 ? `${name.slice(0, dot)} (1)${name.slice(dot)}` : `${name} (1)`;
                  return {
                    index,
                    name,
                    destination: destDir === '/' ? `/${name}` : `${destDir}/${name}`,
                    sourceSize: 8,
                    existingSize: 8,
                    existingIsDir: false,
                    altName
                  };
                });
              return Promise.resolve({ batchId, files: sources.length, bytes: sources.length * 8, conflicts });
            }
            case 'transfer_commit': {
              const { sessionId, batchId, resolutions } = args as {
                sessionId: number;
                batchId: number;
                resolutions: Array<{ index: number; action: string }>;
              };
              const batch = batches[batchId];
              const target = batch.direction === 'upload' ? remote : local;
              const items = batch.sources.flatMap((src, index) => {
                let name = baseName(src);
                const exists = (target[batch.destDir] ?? []).some((e) => e.name === name);
                if (exists) {
                  const answer = resolutions.find((r) => r.index === index)?.action ?? 'skip';
                  if (answer === 'skip') return [];
                  if (answer === 'keepBoth') {
                    const dot = name.lastIndexOf('.');
                    name = dot > 0 ? `${name.slice(0, dot)} (1)${name.slice(dot)}` : `${name} (1)`;
                  }
                }
                const dest = batch.destDir === '/' ? `/${name}` : `${batch.destDir}/${name}`;
                const id = ++nextTransfer;
                transfersLog.push({ id, direction: batch.direction, name });
                // Like the engine's reporter, never report progress after completion.
                let finished = false;
                setTimeout(() => {
                  if (!finished) {
                    fireEvent('transfers-updated', { sessionId, updates: [{ id, state: 'running', done: 4, total: 8 }] });
                  }
                }, 0);
                completions.push(() => {
                  finished = true;
                  addFile(target, batch.destDir, name);
                  fireEvent('transfers-updated', { sessionId, updates: [{ id, state: 'done', done: 8, total: 8 }] });
                });
                return [
                  {
                    id,
                    direction: batch.direction,
                    name,
                    local: batch.direction === 'upload' ? src : dest,
                    remote: batch.direction === 'upload' ? dest : src,
                    size: 8
                  }
                ];
              });
              return Promise.resolve(items);
            }
            case 'edit_confirm_upload':
              opened.push({ cmd, path: `${args.remotePath}:${args.upload}` });
              return Promise.resolve(null);
            case 'open_local_file':
            case 'edit_remote_file':
              opened.push({ cmd, path: (args.path ?? args.remotePath) as string });
              return Promise.resolve(null);
            case 'sftp_close':
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
  await expect(page.getByText('2 hosts')).toBeVisible();
}

test('host-first: a card’s files opens SFTP and browses both sides', async ({ page }) => {
  await boot(page);

  // Host-first spawn — a Dashboard card's `files`, no picker (tech-gui.md §2, §3.2).
  await page.getByTitle('files on web-1').click();

  await expect(page.getByRole('button', { name: 'web-1 · sftp', exact: true })).toBeVisible();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });

  // Both panes list their own filesystem, from distinct commands (local direct, remote event).
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();
});

function completeTransfer(page: Page): Promise<void> {
  return page.evaluate(() => (window as unknown as { __completeTransfer: () => void }).__completeTransfer());
}

test('round-trip: upload a local file to the remote, then download a remote file', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();

  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  const queue = page.getByRole('region', { name: 'Transfers' });
  await expect(localPane.getByText('notes.txt')).toBeVisible();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  // Upload: select the local file, click Upload — it joins the background queue.
  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(queue.getByText('1 transfer in progress')).toBeVisible();

  // The panes stay usable while it runs: browse into a remote folder and back.
  await remotePane.getByRole('option', { name: 'var' }).dblclick();
  await expect(remotePane.getByText('/var')).toBeVisible();
  await remotePane.getByRole('option', { name: '..' }).click();
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  // Complete it: the remote pane showing the destination re-lists with the new file.
  await completeTransfer(page);
  await expect(queue.getByText('All transfers finished')).toBeVisible();
  await expect(remotePane.getByText('notes.txt')).toBeVisible();

  // Download: select a remote file, click Download, complete — the local pane re-lists it.
  await remotePane.getByRole('checkbox', { name: 'Mark config.yml' }).click();
  await page.getByRole('button', { name: 'Download' }).click();
  await expect(queue.getByText('1 transfer in progress')).toBeVisible();
  await completeTransfer(page);
  await expect(localPane.getByText('config.yml')).toBeVisible();

  // Clearing the finished transfers hides the queue.
  await queue.getByRole('button', { name: 'Clear' }).click();
  await expect(queue).toHaveCount(0);
});

test('a name conflict asks first; “keep both” saves under a numbered name', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });

  // Put notes.txt on the server first.
  await localPane.getByRole('checkbox', { name: 'Mark notes.txt' }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  await completeTransfer(page);
  await expect(remotePane.getByText('notes.txt')).toBeVisible();

  // Upload it again: the conflict dialog asks.
  await page.getByRole('button', { name: 'Upload' }).click();
  const dialog = page.getByRole('dialog', { name: 'File already exists' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Keep both' }).click();
  await expect(dialog).toHaveCount(0);
  await completeTransfer(page);
  await expect(remotePane.getByText('notes (1).txt')).toBeVisible();

  // Cancel backs out of the whole batch: nothing is queued.
  await page.getByRole('button', { name: 'Upload' }).click();
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(page.getByRole('region', { name: 'Transfers' }).getByText('in progress')).toHaveCount(0);
});

test('dragging a remote file onto the local pane downloads it', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });
  const source = remotePane.getByRole('option', { name: 'config.yml' });
  await expect(source).toBeVisible();

  const from = await source.boundingBox();
  const to = await localPane.getByRole('listbox').boundingBox();
  if (!from || !to) throw new Error('panes not laid out');
  await page.mouse.move(from.x + 20, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + 60, from.y + 40, { steps: 4 });
  await page.mouse.move(to.x + to.width / 2, to.y + to.height - 20, { steps: 8 });
  // The drag ghost names the target: the local pane's current folder.
  await expect(page.getByText('→ here')).toBeVisible();
  await page.mouse.up();

  await expect(page.getByRole('region', { name: 'Transfers' }).getByText('1 transfer in progress')).toBeVisible();
  await completeTransfer(page);
  await expect(localPane.getByText('config.yml')).toBeVisible();
});

test('double-clicking a file opens it in the editor', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const localPane = page.getByRole('region', { name: 'Local' });
  const remotePane = page.getByRole('region', { name: 'web-1' });

  await localPane.getByRole('option', { name: 'notes.txt' }).dblclick();
  await remotePane.getByRole('option', { name: 'config.yml' }).dblclick();
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __opened: unknown[] }).__opened))
    .toEqual([
      { cmd: 'open_local_file', path: '/home/user/notes.txt' },
      { cmd: 'edit_remote_file', path: '/config.yml' }
    ]);
});

test('an inactive tab’s modal never overlays another entity (§2 exactly-one-active)', async ({
  page
}) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  await expect(page.getByRole('region', { name: 'web-1' })).toBeVisible();
  // Return to the Dashboard (a session hides it) to open a second SFTP session.
  await page.getByRole('button', { name: 'Dashboard', exact: true }).click();
  await page.getByTitle('files on db-1').click();
  await expect(page.getByRole('region', { name: 'db-1' })).toBeVisible();

  // Open db-1's New-folder modal. The modal scrim traps the sidebar, so the ⌘K
  // navigator is the reachable way to switch entity while a modal is open.
  await page.getByRole('button', { name: 'Folder' }).click();
  await expect(page.getByRole('dialog', { name: 'New folder' })).toBeVisible();
  await page.keyboard.press('Control+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await palette.getByRole('textbox').fill('web-1');
  await page.keyboard.press('Enter');

  // Activating the web-1 session must fully hide db-1's modal — never two at once.
  await expect(page.getByRole('region', { name: 'web-1' })).toBeVisible();
  await expect(page.getByRole('dialog', { name: 'New folder' })).toHaveCount(0);
});

test('action-first: the SFTP spawner opens the host picker, then a live session', async ({
  page
}) => {
  await boot(page);

  await page.getByRole('button', { name: 'SFTP', exact: true }).click();
  await page.getByRole('dialog').getByText('web-1', { exact: true }).click();

  await expect(page.getByRole('button', { name: 'web-1 · sftp', exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'web-1' }).getByText('config.yml')).toBeVisible();
});

test('saving an edited server file asks before uploading it', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  await expect(page.getByRole('region', { name: 'web-1' }).getByText('config.yml')).toBeVisible();

  // The backend saw a settled save of the local copy.
  await page.evaluate(() =>
    (window as unknown as { __fire: (e: string, p: unknown) => void }).__fire('edit-sync', {
      sessionId: 1,
      remotePath: '/config.yml',
      state: 'modified'
    })
  );
  const ask = page.getByRole('dialog', { name: 'File was changed' });
  await expect(ask).toBeVisible();
  await expect(ask.getByText('“config.yml” was changed')).toBeVisible();
  await ask.getByRole('button', { name: 'Upload', exact: true }).click();
  await expect(ask).toHaveCount(0);
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __opened: unknown[] }).__opened))
    .toContainEqual({ cmd: 'edit_confirm_upload', path: '/config.yml:true' });
});

test('a terminal docks under the SFTP panes', async ({ page }) => {
  await boot(page);
  await page.getByTitle('files on web-1').click();
  const remotePane = page.getByRole('region', { name: 'web-1' });
  await expect(remotePane.getByText('config.yml')).toBeVisible();

  await remotePane.getByRole('button', { name: 'Terminal' }).click();
  const dock = page.getByRole('region', { name: 'Terminal' });
  await expect(dock).toBeVisible();
  await expect(dock.locator('.xterm')).toBeVisible();
  // Hiding keeps the shell; the toolbar button brings it back.
  await dock.getByRole('button', { name: 'Hide terminal' }).click();
  await expect(dock).toBeHidden();
  await page.keyboard.press('Control+Backquote');
  await expect(dock).toBeVisible();
  await dock.getByRole('button', { name: 'Close terminal' }).click();
  await expect(page.getByRole('region', { name: 'Terminal' })).toHaveCount(0);
});
