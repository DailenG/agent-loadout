// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Ported from DailenG/skilldock@181b52b src/features/skills/components/SkillSourceSwitcher.tsx:
// the tabs row (managed library first, then the first tools, then "More" with a menu). Sources
// here are the store, each client's skill folder in the page's scope, and the agents CLI.

import { useEffect, useRef, useState } from "react";
import { toolLogoUrl } from "@/app/components/ToolLogo";
import { t } from "@/i18n/en";
import { ManagedLibraryIcon } from "./icons";
import { monogram } from "./labels";

const MAX_VISIBLE_TOOL_SOURCES = 5;

/** The store tab. */
export const STORE_SOURCE = "store";
/** The agents CLI tab (SPEC 12.4). */
export const AGENTS_SOURCE = "agents-cli";

export type SourceTab = {
  id: string;
  name: string;
  count: number;
};

function SourceLogo({ tab }: { tab: SourceTab }) {
  const [failed, setFailed] = useState(false);
  const url = toolLogoUrl(tab.id);
  return (
    <span className="skills-source-tab__logo" aria-hidden="true">
      {url && !failed ? <img src={url} alt="" loading="lazy" onError={() => setFailed(true)} /> : monogram(tab.name)}
    </span>
  );
}

type SkillSourceSwitcherProps = {
  active: string;
  storeCount: number;
  tools: SourceTab[];
  onChange: (id: string) => void;
};

export function SkillSourceSwitcher({ active, storeCount, tools, onChange }: SkillSourceSwitcherProps) {
  const [isMenuOpen, setIsMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const activeTool = tools.find((tool) => tool.id === active);
  let visible = tools.slice(0, MAX_VISIBLE_TOOL_SOURCES);
  if (activeTool && !visible.includes(activeTool)) {
    visible = [...visible.slice(0, MAX_VISIBLE_TOOL_SOURCES - 1), activeTool];
  }
  const hidden = tools.filter((tool) => !visible.includes(tool));

  useEffect(() => {
    if (!isMenuOpen) {
      return;
    }
    function handlePointerDown(event: PointerEvent) {
      if (!menuRef.current?.contains(event.target as Node)) {
        setIsMenuOpen(false);
      }
    }
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setIsMenuOpen(false);
      }
    }
    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [isMenuOpen]);

  function choose(id: string) {
    setIsMenuOpen(false);
    onChange(id);
  }

  function toolButton(tool: SourceTab, className: string) {
    const isMenuItem = className.includes("menu-item");
    const selected = tool.id === active;
    return (
      <button
        key={tool.id}
        className={`${className}${selected ? " is-selected" : ""}`}
        type="button"
        role={isMenuItem ? "menuitem" : "tab"}
        aria-selected={isMenuItem ? undefined : selected}
        aria-label={`${tool.name} ${tool.count}`}
        title={tool.name}
        onClick={() => choose(tool.id)}
      >
        <SourceLogo tab={tool} />
        {isMenuItem ? <span>{tool.name}</span> : null}
        <span className="skills-source-tab__count">{tool.count}</span>
      </button>
    );
  }

  return (
    <div className="skills-source-header">
      <div className="skills-source-tabs-row skills-source-tabs-row--tabs">
        <div className="skills-source-tabs" role="tablist" aria-label={t("skills.source.tabs.aria")}>
          <button
            className={`skills-source-tab${active === STORE_SOURCE ? " is-selected" : ""}`}
            type="button"
            role="tab"
            aria-selected={active === STORE_SOURCE}
            title={t("skills.source.managedLibrary")}
            onClick={() => choose(STORE_SOURCE)}
          >
            <span className="skills-source-tab__logo skills-source-tab__logo--managed" aria-hidden="true">
              <ManagedLibraryIcon />
            </span>
            <span>{t("skills.source.managedLibrary")}</span>
            <span className="skills-source-tab__count">{storeCount}</span>
          </button>
          {visible.map((tool) => toolButton(tool, "skills-source-tab"))}
          {hidden.length > 0 ? (
            <div className="skills-source-menu" ref={menuRef}>
              <button
                className="skills-source-more-button"
                type="button"
                aria-haspopup="menu"
                aria-expanded={isMenuOpen}
                onClick={() => setIsMenuOpen((open) => !open)}
              >
                {t("skills.source.more", { count: hidden.length })}
                <span aria-hidden="true">⌄</span>
              </button>
              {isMenuOpen ? (
                <div className="skills-source-menu__popover skills-source-menu__popover--right" role="menu">
                  {hidden.map((tool) => toolButton(tool, "skills-source-menu-item"))}
                </div>
              ) : null}
            </div>
          ) : null}
        </div>
      </div>
      <div className="skills-source-divider" aria-hidden="true" />
    </div>
  );
}
