// Light, dark and system themes (SPEC 13.1). The preference is a local UI
// choice kept in the webview's storage; `data-theme` on <html> selects the
// token set in tokens.css.

import { useCallback, useEffect, useState } from "react";
import { BIN_NAME } from "@/ipc/bindings";

export type ThemePreference = "light" | "dark" | "system";

export const THEME_STORAGE_KEY = `${BIN_NAME}.settings.theme`;

const DARK_QUERY = "(prefers-color-scheme: dark)";

export function readThemePreference(): ThemePreference {
  const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
  return stored === "light" || stored === "dark" ? stored : "system";
}

export function resolveTheme(preference: ThemePreference): "light" | "dark" {
  if (preference !== "system") {
    return preference;
  }
  return window.matchMedia?.(DARK_QUERY).matches ? "dark" : "light";
}

/** The current preference and its setter; keeps `data-theme` in sync. */
export function useThemePreference(): [ThemePreference, (preference: ThemePreference) => void] {
  const [preference, setPreferenceState] = useState<ThemePreference>(readThemePreference);

  useEffect(() => {
    const apply = () => {
      document.documentElement.dataset.theme = resolveTheme(preference);
    };
    apply();
    if (preference !== "system" || !window.matchMedia) {
      return;
    }
    const query = window.matchMedia(DARK_QUERY);
    query.addEventListener("change", apply);
    return () => query.removeEventListener("change", apply);
  }, [preference]);

  const setPreference = useCallback((next: ThemePreference) => {
    if (next === "system") {
      window.localStorage.removeItem(THEME_STORAGE_KEY);
    } else {
      window.localStorage.setItem(THEME_STORAGE_KEY, next);
    }
    setPreferenceState(next);
  }, []);

  return [preference, setPreference];
}
