// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// `McpToolLogo` and the logo manifest ported from DailenG/skilldock@181b52b
// src/app/routes/mcp.tsx and src/features/skills/utils/tool-logo-manifest.json;
// keys are Loadout client ids. Logos are the clients' trademarks, used to
// identify them (SPEC 18).

import { useState } from "react";

const TOOL_LOGOS: Record<string, string> = {
  "claude-code": "/tool-logos/claude-code.png",
  "claude-desktop": "/tool-logos/claude-code.png",
  codex: "/tool-logos/codex.png",
  pi: "/tool-logos/pi.svg",
  omp: "/tool-logos/omp.svg",
  grok: "/tool-logos/grok.svg",
  "mimo-code": "/tool-logos/mimo-code.svg",
  workbuddy: "/tool-logos/workbuddy.svg",
  zcode: "/tool-logos/zcode.png",
  opencode: "/tool-logos/opencode.png",
  cursor: "/tool-logos/cursor.png",
  gemini: "/tool-logos/gemini.svg",
  antigravity: "/tool-logos/antigravity.png",
  windsurf: "/tool-logos/windsurf.png",
  devin: "/tool-logos/devin.png",
  intellij: "/tool-logos/intellij.svg",
  openclaw: "/tool-logos/openclaw.png",
  continue: "/tool-logos/continue.png",
  iflow: "/tool-logos/iflow.png",
  codebuddy: "/tool-logos/codebuddy.svg",
  trae: "/tool-logos/trae.png",
  "trae-cn": "/tool-logos/trae-cn.png",
  droid: "/tool-logos/droid.png",
  augment: "/tool-logos/augment.svg",
  cline: "/tool-logos/cline.png",
  commandcode: "/tool-logos/commandcode.png",
  crush: "/tool-logos/crush.png",
  goose: "/tool-logos/goose.ico",
  junie: "/tool-logos/junie.png",
  "kilo-code": "/tool-logos/kilo-code.png",
  kiro: "/tool-logos/kiro.png",
  qoder: "/tool-logos/qoder.svg",
  "qwen-code": "/tool-logos/qwen-code.ico",
  "roo-code": "/tool-logos/roo-code.ico",
  zencoder: "/tool-logos/zencoder.png",
  hermes: "/tool-logos/hermes.png",
  copilot: "/tool-logos/github-copilot.svg",
  "vscode-legacy": "/tool-logos/github-copilot.svg",
};

/** A client's logo, or its first letter when there is none. */
export function ToolLogo({ clientId, name }: { clientId: string; name: string }) {
  const [failed, setFailed] = useState(false);
  const url = TOOL_LOGOS[clientId];
  if (!url || failed) {
    return (
      <span className="mcp-tool-logo" aria-hidden="true">
        {name.slice(0, 1).toUpperCase()}
      </span>
    );
  }
  return (
    <span className="mcp-tool-logo" aria-hidden="true">
      <img src={url} alt="" loading="lazy" onError={() => setFailed(true)} />
    </span>
  );
}
