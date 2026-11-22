// Manual Refresh (SPEC 8.5): rechecks the client files, manifests and skill
// folders behind every view drift shows up in. Markup and styling follow the
// MCP toolbar's refresh button.

import { useState } from "react";
import { useRefreshDrift } from "@/app/apply/driftTriggers";
import { RefreshIcon } from "@/app/routes/mcp/icons";

export function RefreshButton({ label }: { label: string }) {
  const refresh = useRefreshDrift();
  const [isRefreshing, setIsRefreshing] = useState(false);

  async function handleClick() {
    setIsRefreshing(true);
    try {
      await refresh();
    } finally {
      setIsRefreshing(false);
    }
  }

  return (
    <button
      className={`secondary-button secondary-button--compact skills-toolbar-button skills-toolbar-button--refresh${isRefreshing ? " is-loading" : ""}`}
      type="button"
      onClick={() => void handleClick()}
      disabled={isRefreshing}
    >
      <span aria-hidden="true" className="skills-toolbar-button__icon">
        <RefreshIcon isSpinning={isRefreshing} />
      </span>
      <span>{label}</span>
    </button>
  );
}
