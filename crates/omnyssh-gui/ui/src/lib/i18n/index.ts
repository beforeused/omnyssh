import { derived, get } from 'svelte/store';
import { createPref } from '$lib/stores/settings';
import { en } from './en';
import { ru } from './ru';

// UI translations. Dictionaries are flat `section.key` maps; `en` is the source of
// truth and `ru` is typed against its keys, so a missing translation fails
// svelte-check. Values may carry `{name}` placeholders, and a plural entry picks its
// form from `count` with the language's own rules (Russian has three).

export type Locale = 'en' | 'ru';
export const LOCALES: ReadonlyArray<{ id: Locale; label: string }> = [
  { id: 'en', label: 'English' },
  { id: 'ru', label: 'Русский' }
];

export type Plural = { one: string; few?: string; many?: string; other: string };
export type Message = string | Plural;
export type MessageKey = keyof typeof en;
export type Params = Record<string, string | number>;

const DICTS: Record<Locale, Record<MessageKey, Message>> = { en, ru };

function systemLocale(): Locale {
  try {
    return navigator.language?.toLowerCase().startsWith('ru') ? 'ru' : 'en';
  } catch {
    return 'en';
  }
}

export function coerceLocale(raw: unknown): Locale {
  return raw === 'en' || raw === 'ru' ? raw : systemLocale();
}

/** The chosen UI language (Settings → Appearance). Defaults to the system's. */
export const locale = createPref<Locale>('omnyssh-locale', 'locale', systemLocale(), coerceLocale);

const pluralRules = new Map<Locale, Intl.PluralRules>();
function pluralForm(l: Locale, n: number): 'one' | 'few' | 'many' | 'other' {
  let rules = pluralRules.get(l);
  if (!rules) {
    rules = new Intl.PluralRules(l);
    pluralRules.set(l, rules);
  }
  const form = rules.select(n);
  return form === 'one' || form === 'few' || form === 'many' ? form : 'other';
}

/** Translate `key` into `l`, filling `{placeholders}` from `params`. */
export function translate(l: Locale, key: MessageKey, params?: Params): string {
  const msg = DICTS[l][key] ?? en[key];
  let text: string;
  if (typeof msg === 'string') {
    text = msg;
  } else {
    const n = Number(params?.count ?? 0);
    const form = pluralForm(l, n);
    text = msg[form] ?? msg.other;
  }
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in params ? String(params[name]) : whole
  );
}

/** `$t('key', { name })` in components. */
export const t = derived(locale, (l) => (key: MessageKey, params?: Params) => translate(l, key, params));

/** For non-component code: translate with the current language. */
export function tr(key: MessageKey, params?: Params): string {
  return translate(get(locale), key, params);
}
