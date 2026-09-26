// Thin typed wrappers over the generated command bindings (tech-gui.md §3.5).
// Components call these, never `invoke` directly.

import type { Channel } from '@tauri-apps/api/core';
import { commands } from '$lib/bindings';
import type {
  AuthModeDto,
  DockerActionDto,
  DockerListDto,
  LocalFsOpDto,
  RemoteFsOpDto,
  VpnStatusDto,
  ConflictResolutionDto,
  HostAuthDto,
  KeyChoiceDto,
  SshKeyDto,
  EditorAppDto,
  EditorDto,
  FileEntryDto,
  PreparedBatchDto,
  TransferDirectionDto,
  TransferItemDto,
  HostDto,
  HostInputDto,
  SnippetDto,
  TerminalBytes,
  UpdateConfigDto,
  UpdateInfoDto
} from '$lib/bindings';

export async function listHosts(): Promise<HostDto[]> {
  const res = await commands.listHosts();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Reload hosts from disk and (re)start the pollers; broadcasts `hosts-loaded`. */
export async function reloadHosts(): Promise<void> {
  const res = await commands.reloadHosts();
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Add or edit a manual host in `hosts.toml`. Call `reloadHosts` after to refresh. */
export async function saveHost(input: HostInputDto): Promise<void> {
  const res = await commands.saveHost(input);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Delete a manual host by name. A missing / SSH-config name is a no-op success. */
export async function deleteHost(name: string): Promise<void> {
  const res = await commands.deleteHost(name);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Read the saved snippets from the shared `snippets.toml`. */
export async function listSnippets(): Promise<SnippetDto[]> {
  const res = await commands.listSnippets();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Upsert one snippet by name and persist the whole list. */
export async function saveSnippet(snippet: SnippetDto): Promise<void> {
  const res = await commands.saveSnippet(snippet);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Delete the snippet named `name` and persist. */
export async function deleteSnippet(name: string): Promise<void> {
  const res = await commands.deleteSnippet(name);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Run a snippet on one or more hosts; results arrive as `snippet-result` events. */
export async function executeSnippet(
  snippetName: string,
  hostNames: string[],
  params: Record<string, string>
): Promise<void> {
  const res = await commands.executeSnippet(snippetName, hostNames, params);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Open a terminal for `hostName`, streaming raw output into `onOutput`; returns the
 *  public session id used by the write/resize/close wrappers (tech-gui.md §3.3/§4.2). */
export async function terminalOpen(
  hostName: string,
  cols: number,
  rows: number,
  onOutput: Channel<TerminalBytes>
): Promise<number> {
  const res = await commands.terminalOpen(hostName, cols, rows, onOutput);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Send keystrokes (UTF-8 bytes) to a terminal. */
export async function terminalWrite(sessionId: number, data: number[]): Promise<void> {
  const res = await commands.terminalWrite(sessionId, data);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Reflow a terminal to `cols` x `rows`. */
export async function terminalResize(sessionId: number, cols: number, rows: number): Promise<void> {
  const res = await commands.terminalResize(sessionId, cols, rows);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Close a terminal and its connection. Idempotent for an already-closed id. */
export async function terminalClose(sessionId: number): Promise<void> {
  const res = await commands.terminalClose(sessionId);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Open an SFTP session for `hostName`; returns the public session id the sftp_*
 *  wrappers use, and the tab's `sftp-*` events carry (tech-gui.md §3.4/§4.2). */
export async function sftpOpen(hostName: string): Promise<number> {
  const res = await commands.sftpOpen(hostName);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** List a remote directory; the result arrives as `sftp-dir-listed`. */
export async function sftpList(sessionId: number, path: string): Promise<void> {
  const res = await commands.sftpList(sessionId, path);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Create a remote directory; completion arrives as `sftp-op-done`. */
export async function sftpMkdir(sessionId: number, path: string): Promise<void> {
  const res = await commands.sftpMkdir(sessionId, path);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Rename / move a remote path; completion arrives as `sftp-op-done`. */
export async function sftpRename(sessionId: number, from: string, to: string): Promise<void> {
  const res = await commands.sftpRename(sessionId, from, to);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Delete a remote file (or empty directory); completion arrives as `sftp-op-done`. */
export async function sftpDelete(sessionId: number, path: string): Promise<void> {
  const res = await commands.sftpDelete(sessionId, path);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Request a remote file preview; the bytes arrive as `file-preview`. */
export async function sftpPreview(sessionId: number, path: string): Promise<void> {
  const res = await commands.sftpPreview(sessionId, path);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Close an SFTP session and its connection. Idempotent for an already-closed id. */
export async function sftpClose(sessionId: number): Promise<void> {
  const res = await commands.sftpClose(sessionId);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** List a local directory (returns directly — no event). */
export async function listLocalDir(path: string): Promise<FileEntryDto[]> {
  const res = await commands.listLocalDir(path);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Read up to 4 KiB of a local file as UTF-8 for preview. */
export async function previewLocalFile(path: string): Promise<string> {
  const res = await commands.previewLocalFile(path);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Start auto SSH-key setup for a host; progress + the outcome arrive as `key-setup-*`
 *  events (tech-gui.md §4.2). Fire-and-forget — only an unknown host rejects here. */
export async function startKeySetup(
  hostName: string,
  key: KeyChoiceDto,
  mode: AuthModeDto
): Promise<void> {
  const res = await commands.startKeySetup(hostName, key, mode);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Whether this encrypted key still needs to be unlocked in the current app process. */
export async function keyPassphraseRequired(path: string): Promise<boolean> {
  const res = await commands.keyPassphraseRequired(path);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Validate a key passphrase and retain it in backend process memory only. */
export async function unlockSshKey(path: string, passphrase: string): Promise<void> {
  const res = await commands.unlockSshKey(path, passphrase);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Force an immediate metric poll of every host (tech-gui.md §4.2). */
export async function refreshMetrics(): Promise<void> {
  const res = await commands.refreshMetrics();
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Check GitHub for a newer release; `null` means up to date (tech-gui.md §4.2). */
export async function checkUpdate(): Promise<UpdateInfoDto | null> {
  const res = await commands.checkUpdate();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Download and install the latest desktop bundle (tech-gui.md §4.3). Fully wired once
 *  Stage 5 configures the updater endpoints; until then it reports "not available yet". */
export async function installUpdate(): Promise<void> {
  const res = await commands.installUpdate();
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Read the update-checker preferences from the shared config (tech-gui.md §4.3). */
export async function loadUpdateConfig(): Promise<UpdateConfigDto> {
  const res = await commands.loadUpdateConfig();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Persist the update-checker preferences to the shared config (tech-gui.md §4.3). */
export async function saveUpdateConfig(config: UpdateConfigDto): Promise<void> {
  const res = await commands.saveUpdateConfig(config);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Expand `sources` into a transfer batch targeting `destDir` and report which
 *  destinations already exist. Commit or discard the batch afterwards. */
export async function transferPrepare(
  sessionId: number,
  direction: TransferDirectionDto,
  sources: string[],
  destDir: string
): Promise<PreparedBatchDto> {
  const res = await commands.transferPrepare(sessionId, direction, sources, destDir);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Enqueue a prepared batch with the user's conflict answers; returns its transfers. */
export async function transferCommit(
  sessionId: number,
  batchId: number,
  resolutions: ConflictResolutionDto[]
): Promise<TransferItemDto[]> {
  const res = await commands.transferCommit(sessionId, batchId, resolutions);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Drop a prepared batch the user backed out of. */
export async function transferDiscard(sessionId: number, batchId: number): Promise<void> {
  const res = await commands.transferDiscard(sessionId, batchId);
  if (res.status === 'error') throw new Error(res.error.message);
}

export async function transferCancel(sessionId: number, ids: number[]): Promise<void> {
  const res = await commands.transferCancel(sessionId, ids);
  if (res.status === 'error') throw new Error(res.error.message);
}

export async function transferRetry(sessionId: number, ids: number[]): Promise<void> {
  const res = await commands.transferRetry(sessionId, ids);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Forget transfers cleared from the queue panel. */
export async function transferForget(sessionId: number, ids: number[]): Promise<void> {
  const res = await commands.transferForget(sessionId, ids);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Parallel connections per host for transfers (1–8). */
export async function setTransferStreams(streams: number): Promise<void> {
  const res = await commands.setTransferStreams(streams);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Editors installed on this machine. */
export async function detectEditors(): Promise<EditorAppDto[]> {
  const res = await commands.detectEditors();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Open a local file in `editor`. */
export async function openLocalFile(path: string, editor: EditorDto): Promise<void> {
  const res = await commands.openLocalFile(path, editor);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Download a remote file and open it in `editor`; saves sync back (`edit-sync`). */
export async function editRemoteFile(
  sessionId: number,
  remotePath: string,
  editor: EditorDto
): Promise<void> {
  const res = await commands.editRemoteFile(sessionId, remotePath, editor);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Answer an edit conflict: overwrite the server copy, or reload it locally. */
export async function editResolveConflict(
  sessionId: number,
  remotePath: string,
  overwrite: boolean
): Promise<void> {
  const res = await commands.editResolveConflict(sessionId, remotePath, overwrite);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Answer "upload this save?" for a remote file open in an editor. */
export async function editConfirmUpload(
  sessionId: number,
  remotePath: string,
  upload: boolean
): Promise<void> {
  const res = await commands.editConfirmUpload(sessionId, remotePath, upload);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Private keys found in `~/.ssh`. */
export async function listSshKeys(): Promise<SshKeyDto[]> {
  const res = await commands.listSshKeys();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** Describe a hand-picked key file; rejects when it is not a private key. */
export async function inspectSshKey(path: string): Promise<SshKeyDto> {
  const res = await commands.inspectSshKey(path);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

export async function getDefaultKey(): Promise<string | null> {
  const res = await commands.getDefaultKey();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

export async function setDefaultKey(path: string | null): Promise<void> {
  const res = await commands.setDefaultKey(path);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Which key a host uses and whether a password is stored (never the password). */
export async function hostAuth(hostName: string): Promise<HostAuthDto> {
  const res = await commands.hostAuth(hostName);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

/** A file operation SFTP has no verb for, run on the SFTP tab's server. */
export async function remoteFsOp(sessionId: number, op: RemoteFsOpDto): Promise<void> {
  const res = await commands.remoteFsOp(sessionId, op);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** A file operation on this machine (new folder/file, rename, move to Trash). */
export async function localFsOp(op: LocalFsOpDto): Promise<void> {
  const res = await commands.localFsOp(op);
  if (res.status === 'error') throw new Error(res.error.message);
}

export async function dockerList(hostName: string): Promise<DockerListDto> {
  const res = await commands.dockerList(hostName);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

export async function dockerAction(hostName: string, id: string, action: DockerActionDto): Promise<void> {
  const res = await commands.dockerAction(hostName, id, action);
  if (res.status === 'error') throw new Error(res.error.message);
}

export async function dockerLogs(hostName: string, id: string, tail: number): Promise<string> {
  const res = await commands.dockerLogs(hostName, id, tail);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

export async function dockerShellCommand(id: string, sudo: boolean): Promise<string> {
  const res = await commands.dockerShellCommand(id, sudo);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

export async function vpnStatus(): Promise<VpnStatusDto> {
  const res = await commands.vpnStatus();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

export async function vpnConfigurations(): Promise<string[]> {
  const res = await commands.vpnConfigurations();
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

export async function vpnImport(path: string): Promise<void> {
  const res = await commands.vpnImport(path);
  if (res.status === 'error') throw new Error(res.error.message);
}

export async function vpnState(name: string): Promise<string> {
  const res = await commands.vpnState(name);
  if (res.status === 'error') throw new Error(res.error.message);
  return res.data;
}

export async function vpnConnect(name: string): Promise<void> {
  const res = await commands.vpnConnect(name);
  if (res.status === 'error') throw new Error(res.error.message);
}

export async function vpnDisconnect(name: string): Promise<void> {
  const res = await commands.vpnDisconnect(name);
  if (res.status === 'error') throw new Error(res.error.message);
}

/** Download, verify and install Tunnelblick (progress via `vpn-install-progress`). */
export async function vpnInstall(): Promise<void> {
  const res = await commands.vpnInstall();
  if (res.status === 'error') throw new Error(res.error.message);
}
