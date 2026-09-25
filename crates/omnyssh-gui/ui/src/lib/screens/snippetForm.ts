// Pure snippet form + list logic (tech-gui.md §2.2), kept free of Svelte components
// so it is unit-testable; `Snippets.svelte`/`SnippetEditor.svelte` render it. The
// validation mirrors the TUI's `SnippetForm::to_snippet` so both frontends produce
// the same `snippets.toml` shape.

import { tr } from '$lib/i18n';
import type { SnippetDto, SnippetScopeDto } from '$lib/bindings';

/** The editable form fields (tags/params are comma-separated raw text). */
export interface SnippetFormFields {
  name: string;
  command: string;
  scope: SnippetScopeDto;
  host: string;
  tags: string;
  params: string;
}

export function emptyForm(): SnippetFormFields {
  return { name: '', command: '', scope: 'global', host: '', tags: '', params: '' };
}

export function formFromSnippet(s: SnippetDto): SnippetFormFields {
  return {
    name: s.name,
    command: s.command,
    scope: s.scope,
    host: s.host ?? '',
    tags: (s.tags ?? []).join(', '),
    params: (s.params ?? []).join(', ')
  };
}

function splitCsv(raw: string): string[] {
  return raw
    .split(',')
    .map((t) => t.trim())
    .filter(Boolean);
}

export type FormResult = { ok: true; snippet: SnippetDto } | { ok: false; error: string };

/** Validate + build a `SnippetDto`, or return an error message. Mirrors the TUI's
 *  `to_snippet`: name and command are required; host is required when scope is
 *  'host' but kept if provided under 'global'; tags/params are split, trimmed, and
 *  dropped to `undefined` when empty (so the wire form stays sparse, §4.1). */
export function formToSnippet(f: SnippetFormFields): FormResult {
  const name = f.name.trim();
  if (!name) return { ok: false, error: tr('snip.err.name') };
  const command = f.command.trim();
  if (!command) return { ok: false, error: tr('snip.err.command') };
  const scope: SnippetScopeDto = f.scope === 'host' ? 'host' : 'global';
  const host = f.host.trim();
  if (scope === 'host' && !host) {
    return { ok: false, error: "Host is required when scope is 'host'" };
  }
  const tags = splitCsv(f.tags);
  const params = splitCsv(f.params);
  return {
    ok: true,
    snippet: {
      name,
      command,
      scope,
      host: host || undefined,
      tags: tags.length ? tags : undefined,
      params: params.length ? params : undefined
    }
  };
}

/** A snippet's declared parameter names (drives the run dialog's prompts). */
export function declaredParams(s: SnippetDto): string[] {
  return s.params ?? [];
}

/** Case-insensitive substring filter over name / command / tags (mirrors the TUI's
 *  `filter_snippets`). An empty query keeps everything; order is preserved. */
export function filterSnippets(list: SnippetDto[], query: string): SnippetDto[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter(
    (s) =>
      s.name.toLowerCase().includes(q) ||
      s.command.toLowerCase().includes(q) ||
      (s.tags ?? []).some((t) => t.toLowerCase().includes(q))
  );
}
