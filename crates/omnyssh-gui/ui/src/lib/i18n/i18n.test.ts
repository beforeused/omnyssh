import { describe, expect, it } from 'vitest';
import { en } from './en';
import { ru } from './ru';
import { translate, coerceLocale } from './index';

describe('i18n', () => {
  it('fills placeholders and leaves unknown ones visible', () => {
    expect(translate('en', 'sftp.connecting', { host: 'web-1' })).toBe('Connecting to web-1…');
    expect(translate('ru', 'sftp.connecting', { host: 'web-1' })).toBe('Подключение к web-1…');
    // A literal `{file}` in help text survives when no value is given.
    expect(translate('en', 'settings.commandHint')).toContain('{file}');
  });

  it('picks English and Russian plural forms by count', () => {
    expect(translate('en', 'transfers.inProgress', { count: 1 })).toBe('1 transfer in progress');
    expect(translate('en', 'transfers.inProgress', { count: 3 })).toBe('3 transfers in progress');
    expect(translate('ru', 'transfers.inProgress', { count: 1 })).toBe('Идёт 1 передача');
    expect(translate('ru', 'transfers.inProgress', { count: 3 })).toBe('Идут 3 передачи');
    expect(translate('ru', 'transfers.inProgress', { count: 5 })).toBe('Идут 5 передач');
    expect(translate('ru', 'transfers.inProgress', { count: 21 })).toBe('Идёт 21 передача');
    expect(translate('ru', 'status.hosts', { count: 12 })).toBe('12 серверов');
  });

  it('has a Russian string for every English key, and nothing extra', () => {
    expect(Object.keys(ru).sort()).toEqual(Object.keys(en).sort());
    for (const [key, value] of Object.entries(ru)) {
      const english = en[key as keyof typeof en];
      expect(typeof value, key).toBe(typeof english);
      if (typeof value === 'object') expect(value.one && value.many, key).toBeTruthy();
    }
  });

  it('falls back to a supported locale', () => {
    expect(coerceLocale('ru')).toBe('ru');
    expect(coerceLocale('en')).toBe('en');
    expect(['en', 'ru']).toContain(coerceLocale('de'));
  });
});
