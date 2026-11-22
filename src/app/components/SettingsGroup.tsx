// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Settings group, form row and theme picker markup ported from
// DailenG/skilldock@181b52b src/app/routes/settings.tsx (`SettingsRoute`, `ThemeIcon`).

import type { ReactNode } from "react";
import type { ThemePreference } from "@/app/components/theme";
import { t, type MessageKey } from "@/i18n/en";

export function SettingsGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settings-group">
      <div className="settings-group__heading">
        <span className="settings-group__bar" aria-hidden="true" />
        <h2 className="settings-group__title">{title}</h2>
      </div>
      <div className="panel-card placeholder-panel settings-panel settings-panel--module">
        <div className="settings-form-list">{children}</div>
      </div>
    </section>
  );
}

export function SettingsItem(props: {
  label: string;
  description: ReactNode;
  readonly?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className={`settings-form-item${props.readonly ? " settings-form-item--readonly" : ""}`}>
      <div className="settings-form-item__copy">
        <span className="settings-form-item__title">{props.label}</span>
        <p>{props.description}</p>
      </div>
      {props.children}
    </div>
  );
}

function ThemeIcon({ theme }: { theme: ThemePreference }) {
  if (theme === "system") {
    return (
      <svg viewBox="0 0 18 18" fill="none" aria-hidden="true">
        <rect x="2.5" y="3" width="13" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.45" />
        <path d="M6.5 15h5M9 12v3" stroke="currentColor" strokeWidth="1.45" strokeLinecap="round" />
      </svg>
    );
  }

  if (theme === "dark") {
    return (
      <svg viewBox="0 0 18 18" fill="none" aria-hidden="true">
        <path
          d="M14.2 11.2A6 6 0 0 1 6.8 3.8 5.6 5.6 0 1 0 14.2 11.2Z"
          stroke="currentColor"
          strokeWidth="1.45"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }

  return (
    <svg viewBox="0 0 18 18" fill="none" aria-hidden="true">
      <circle cx="9" cy="9" r="2.75" stroke="currentColor" strokeWidth="1.45" />
      <path
        d="M9 2.2v1.4M9 14.4v1.4M15.8 9h-1.4M3.6 9H2.2M13.8 4.2l-1 1M5.2 12.8l-1 1M13.8 13.8l-1-1M5.2 5.2l-1-1"
        stroke="currentColor"
        strokeWidth="1.45"
        strokeLinecap="round"
      />
    </svg>
  );
}

const THEME_OPTIONS: { value: ThemePreference; labelKey: MessageKey }[] = [
  { value: "light", labelKey: "settings.theme.option.light" },
  { value: "dark", labelKey: "settings.theme.option.dark" },
  { value: "system", labelKey: "settings.theme.option.system" },
];

export function ThemePicker(props: {
  value: ThemePreference;
  onChange: (value: ThemePreference) => void;
}) {
  return (
    <div className="settings-form-item__control settings-form-item__control--theme">
      <div className="settings-theme-picker" role="group" aria-label={t("settings.theme.label")}>
        {THEME_OPTIONS.map((option) => (
          <button
            key={option.value}
            className={`settings-theme-option${props.value === option.value ? " is-selected" : ""}`}
            type="button"
            aria-pressed={props.value === option.value}
            onClick={() => props.onChange(option.value)}
          >
            <ThemeIcon theme={option.value} />
            <span>{t(option.labelKey)}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
