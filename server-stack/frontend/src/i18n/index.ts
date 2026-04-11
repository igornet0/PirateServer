import {
  TRANSLATIONS,
  type Locale,
  type MessageKey,
} from "./translations.js";

const STORAGE_KEY = "deploy.locale";

const localeListeners = new Set<() => void>();

function readStoredLocale(): Locale | null {
  try {
    const s = localStorage.getItem(STORAGE_KEY);
    if (s === "ru" || s === "en") {
      return s;
    }
  } catch {
    /* ignore */
  }
  return null;
}

function detectBrowserLocale(): Locale {
  if (typeof navigator === "undefined") {
    return "en";
  }
  const lang = navigator.language?.toLowerCase() ?? "";
  return lang.startsWith("ru") ? "ru" : "en";
}

let currentLocale: Locale = readStoredLocale() ?? detectBrowserLocale();

export function getLocale(): Locale {
  return currentLocale;
}

export function setLocale(locale: Locale): void {
  if (locale !== "en" && locale !== "ru") {
    return;
  }
  currentLocale = locale;
  try {
    localStorage.setItem(STORAGE_KEY, locale);
  } catch {
    /* ignore */
  }
  document.documentElement.lang = locale === "ru" ? "ru" : "en";
  applyDocumentTranslations();
  for (const cb of localeListeners) {
    cb();
  }
}

export function onLocaleChange(cb: () => void): () => void {
  localeListeners.add(cb);
  return () => localeListeners.delete(cb);
}

export function t(
  key: MessageKey,
  vars?: Record<string, string | number>,
): string {
  const table = TRANSLATIONS[getLocale()];
  let s = (table[key] ?? TRANSLATIONS.en[key] ?? key) as string;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      s = s.replaceAll(`{${k}}`, String(v));
    }
  }
  return s;
}

function applyI18nElements(root: ParentNode = document): void {
  root.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    const key = el.dataset.i18n as MessageKey | undefined;
    if (!key) {
      return;
    }
    const text = t(key);
    if (el.tagName === "TITLE") {
      document.title = text;
    } else {
      el.textContent = text;
    }
  });

  root.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>(
    "[data-i18n-placeholder]",
  ).forEach((el) => {
    const key = el.dataset.i18nPlaceholder as MessageKey | undefined;
    if (!key) {
      return;
    }
    el.placeholder = t(key);
  });

  root.querySelectorAll<HTMLElement>("[data-i18n-html]").forEach((el) => {
    const key = el.dataset.i18nHtml as MessageKey | undefined;
    if (!key) {
      return;
    }
    el.innerHTML = t(key);
  });

  root.querySelectorAll<HTMLElement>("[data-i18n-aria]").forEach((el) => {
    const key = el.dataset.i18nAria as MessageKey | undefined;
    if (!key) {
      return;
    }
    el.setAttribute("aria-label", t(key));
  });
}

export function applyDocumentTranslations(): void {
  applyI18nElements(document);
}

function bindLanguageSwitcher(): void {
  const select = document.getElementById(
    "lang-switcher",
  ) as HTMLSelectElement | null;
  if (select) {
    select.value = getLocale();
    select.addEventListener("change", () => {
      const v = select.value;
      if (v === "en" || v === "ru") {
        setLocale(v);
      }
    });
  }
}

/** Call once per page (dashboard or login) before other UI logic. */
export function initI18n(): void {
  document.documentElement.lang = getLocale() === "ru" ? "ru" : "en";
  applyDocumentTranslations();
  bindLanguageSwitcher();
}
