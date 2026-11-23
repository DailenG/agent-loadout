// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Ported from DailenG/skilldock@181b52b src/app/components/ToolbarGoInstallButton.tsx.

import { collapsibleAction, HEADER_ACTION_LABEL } from "@/app/components/HeaderSlots";
import { t } from "@/i18n/en";

function GoInstallIcon() {
  return (
    <svg
      className="skills-toolbar-button__svg"
      viewBox="0 0 20 20"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M10 4.25v7.5"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
      />
      <path
        d="m7.25 9.25 2.75 2.75 2.75-2.75"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M5.25 14.75h9.5"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
      />
    </svg>
  );
}

type ToolbarGoInstallButtonProps = {
  onClick: () => void;
};

export function ToolbarGoInstallButton(props: ToolbarGoInstallButtonProps) {
  return (
    <button
      className="secondary-button secondary-button--compact skills-toolbar-button skills-toolbar-button--go-install"
      type="button"
      onClick={props.onClick}
      {...collapsibleAction(t("toolbar.goInstall"), "early")}
    >
      <span aria-hidden="true" className="skills-toolbar-button__icon">
        <GoInstallIcon />
      </span>
      <span className={HEADER_ACTION_LABEL}>{t("toolbar.goInstall")}</span>
    </button>
  );
}
