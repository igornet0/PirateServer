import React, {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";
import ru from "./locales/ru";

export type UiLanguage = "ru" | "en";

type I18nContextValue = {
  language: UiLanguage;
  setLanguage: (next: UiLanguage) => void;
  t: (key: string) => string;
};

const STORAGE_KEY = "pirateDesktop.uiLanguage";

// `ru` is the synchronous fallback locale and is always bundled. Every other
// locale is loaded on demand so its (~45 KB) dictionary is not parsed at
// startup for users who never switch languages.
type Dict = Record<string, string>;

const localeCache: Partial<Record<UiLanguage, Dict>> = { ru };

const localeLoaders: Record<UiLanguage, () => Promise<Dict>> = {
  ru: async () => ru,
  en: async () => (await import("./locales/en")).default,
};

const I18nContext = createContext<I18nContextValue | null>(null);

function detectInitialLanguage(): UiLanguage {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved === "ru" || saved === "en") return saved;
  const nav = navigator.language.toLowerCase();
  return nav.startsWith("ru") ? "ru" : "en";
}

export function I18nProvider({ children }: { children: React.ReactNode }) {
  const [language, setLanguageState] = useState<UiLanguage>(() =>
    detectInitialLanguage(),
  );
  // Active dictionary; starts at whatever is cached (always at least `ru`).
  const [dict, setDict] = useState<Dict>(() => localeCache[language] ?? ru);

  useEffect(() => {
    const cached = localeCache[language];
    if (cached) {
      setDict(cached);
      return;
    }
    let cancelled = false;
    void localeLoaders[language]().then((loaded) => {
      localeCache[language] = loaded;
      if (!cancelled) setDict(loaded);
    });
    return () => {
      cancelled = true;
    };
  }, [language]);

  const value = useMemo<I18nContextValue>(() => {
    return {
      language,
      setLanguage: (next) => {
        setLanguageState(next);
        localStorage.setItem(STORAGE_KEY, next);
      },
      // Until a non-`ru` dictionary finishes loading, `dict` is still `ru`,
      // so `t` transparently falls back instead of showing raw keys.
      t: (key) => dict[key] ?? ru[key] ?? key,
    };
  }, [language, dict]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) {
    throw new Error("useI18n must be used inside I18nProvider");
  }
  return ctx;
}
