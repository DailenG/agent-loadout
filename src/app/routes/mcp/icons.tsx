// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Icons and the server monogram ported from DailenG/skilldock@181b52b src/app/routes/mcp.tsx and
// src/features/skills/components/PowerToggleIcon.tsx; PasteIcon is new.

function monogram(value: string): string {
  const first = Array.from(value.trim()).find((character) => /[\p{L}\p{N}]/u.test(character));
  return (first ?? "?").toLocaleUpperCase();
}

/** A server's first letter with the MCP mark and an active dot. */
export function McpServerMonogram({ name, active }: { name: string; active: boolean }) {
  return (
    <div className="link-badge link-badge--mcp-monogram" aria-hidden="true">
      <span className="link-badge__type-mark link-badge__type-mark--mcp">
        <svg viewBox="0 0 12 12" fill="none">
          <path d="M4.2 4.2 2.8 5.6a2 2 0 0 0 2.8 2.8L7 7" stroke="currentColor" strokeWidth="1.45" strokeLinecap="round" />
          <path d="M7.8 7.8 9.2 6.4a2 2 0 0 0-2.8-2.8L5 5" stroke="currentColor" strokeWidth="1.45" strokeLinecap="round" />
        </svg>
      </span>
      <span className="link-badge__label">{monogram(name)}</span>
      <span className={`link-badge__status-dot ${active ? "is-active" : "is-inactive"}`} />
    </div>
  );
}

export function RefreshIcon({ isSpinning = false }: { isSpinning?: boolean }) {
  return (
    <svg className={isSpinning ? "skills-toolbar-button__svg is-spinning" : "skills-toolbar-button__svg"} viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path d="M16.2 9.1a6.2 6.2 0 0 0-10.7-3.6" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M3.7 3.9v3.7h3.7" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M3.8 10.9a6.2 6.2 0 0 0 10.7 3.6" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M16.3 16.1v-3.7h-3.7" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function AddIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path d="M10 4.75v10.5M4.75 10h10.5" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  );
}

export function PasteIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path
        d="M7.25 4.25h-1.5a1.5 1.5 0 0 0-1.5 1.5v9.5a1.5 1.5 0 0 0 1.5 1.5h8.5a1.5 1.5 0 0 0 1.5-1.5v-9.5a1.5 1.5 0 0 0-1.5-1.5h-1.5"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path d="M7.75 3.25h4.5v2.5h-4.5zM7.5 10h5M7.5 13h3.5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function EditIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path
        d="m4.75 13.25-.5 2.5 2.5-.5 8.15-8.15a1.75 1.75 0 0 0-2.47-2.47L4.75 13.25Z"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path d="m11.75 5.25 3 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

export function DeleteIcon() {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path
        d="M4.75 5.75h10.5M7.25 5.75V4.5c0-.55.45-1 1-1h3.5c.55 0 1 .45 1 1v1.25m-6.25 0 .45 8.25c.03.61.54 1.08 1.15 1.08h3.8c.61 0 1.12-.47 1.15-1.08l.45-8.25M8.5 8.75v4.25m3 0V8.75"
        stroke="currentColor"
        strokeWidth="1.7"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function PowerToggleIcon({ isSpinning = false }: { isSpinning?: boolean }) {
  return (
    <svg
      className={isSpinning ? "skill-card__power-icon is-spinning" : "skill-card__power-icon"}
      viewBox="0 0 20 20"
      fill="none"
      aria-hidden="true"
    >
      <path d="M10 3.5v6" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
      <path d="M6.35 6.85a5.25 5.25 0 1 0 7.3 0" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
    </svg>
  );
}
