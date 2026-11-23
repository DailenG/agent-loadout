// The sidebar scope switcher (SPEC 13.1): User first, then every project with
// its trust state, then Add folder. Choosing an entry sets the active scope;
// Add folder runs the Projects page's folder flow and selects the result.
// With the sidebar collapsed, one icon shows the current scope in its tooltip.

import { createPortal } from "react-dom";
import { AppSelect, type AppSelectOption } from "@/app/components/AppSelect";
import { useAddFolderFlow } from "@/app/routes/projects/AddFolderFlow";
import { TRUST_KEYS } from "@/app/routes/projects/labels";
import { useActiveScope, useProjectList } from "@/app/scope/ActiveScope";
import { t } from "@/i18n/en";
import type { ProjectTrust } from "@/ipc/bindings";

const USER_VALUE = "user";
const HINT_VALUE = "hint";
const ADD_VALUE = "add";
const PROJECT_PREFIX = "project:";

const TRUST_ICON_KIND: Record<ProjectTrust, "trusted" | "review" | "broken"> = {
  trusted: "trusted",
  untrusted: "review",
  changed: "review",
  missing: "broken",
  invalid: "broken",
};

function TrustIcon({ trust }: { trust: ProjectTrust }) {
  const kind = TRUST_ICON_KIND[trust];
  return (
    <span className={`scope-trust scope-trust--${kind}`} role="img" aria-label={t(TRUST_KEYS[trust])}>
      <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
        <path
          d="M8 1.75 2.75 3.6v4.1c0 3.1 2.2 5.5 5.25 6.55 3.05-1.05 5.25-3.45 5.25-6.55V3.6L8 1.75Z"
          stroke="currentColor"
          strokeWidth="1.3"
          strokeLinejoin="round"
        />
        {kind === "trusted" ? (
          <path d="m5.6 8 1.7 1.7 3.1-3.3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
        ) : kind === "review" ? (
          <path d="M8 5v3.2M8 10.6v.1" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
        ) : (
          <path d="m6 6 4 4m0-4-4 4" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        )}
      </svg>
    </span>
  );
}

function UserIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <circle cx="8" cy="5.5" r="2.75" stroke="currentColor" strokeWidth="1.3" />
      <path d="M2.75 13.5c.6-2.4 2.7-4 5.25-4s4.65 1.6 5.25 4" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path
        d="M2 4.25c0-.7.55-1.25 1.25-1.25h3l1.5 1.75h5c.7 0 1.25.55 1.25 1.25v5.75c0 .7-.55 1.25-1.25 1.25h-9.5C2.55 13 2 12.45 2 11.75v-7.5Z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <path d="M8 3.5v9M3.5 8h9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

export function ScopeSwitcher(props: { onExpandSidebar: () => void }) {
  const { scope, setScope, project } = useActiveScope();
  const projects = useProjectList();
  const flow = useAddFolderFlow({ onAdded: (change) => setScope({ kind: "project", id: change.id }) });
  const rows = projects.data ?? [];

  const options: AppSelectOption<string>[] = [
    { value: USER_VALUE, label: t("scope.userOption"), icon: <UserIcon /> },
    ...rows.map((row) => ({
      value: `${PROJECT_PREFIX}${row.id}`,
      label: row.name,
      icon: <TrustIcon trust={row.trust} />,
      tooltip: t("scope.projectTooltip", { path: row.path, trust: t(TRUST_KEYS[row.trust]) }),
    })),
    ...(projects.data && rows.length === 0
      ? [{ value: HINT_VALUE, label: t("scope.noProjects"), disabled: true }]
      : []),
    {
      value: ADD_VALUE,
      label: t("scope.addFolder"),
      icon: <PlusIcon />,
      disabled: flow.picking,
      onSelect: () => flow.start("add"),
    },
  ];

  const value = scope.kind === "user" ? USER_VALUE : `${PROJECT_PREFIX}${scope.id}`;
  const selectedLabel = scope.kind === "user" ? t("scope.user") : (project?.name ?? "");
  const currentName = scope.kind === "user" ? t("scope.userOption") : (project?.name ?? "");
  const collapsedLabel = t("scope.collapsed", { name: currentName });

  function choose(next: string) {
    if (next === USER_VALUE) {
      setScope({ kind: "user" });
    } else if (next.startsWith(PROJECT_PREFIX)) {
      setScope({ kind: "project", id: next.slice(PROJECT_PREFIX.length) });
    }
  }

  return (
    <>
      <div className="scope-switcher">
        <span className="scope-switcher__label" aria-hidden="true">
          {t("scope.label")}
        </span>
        <AppSelect
          className="scope-switcher__select"
          menuClassName="scope-switcher__popover"
          minMenuWidth={240}
          value={value}
          options={options}
          onChange={choose}
          ariaLabel={t("scope.switcher.aria")}
          selectedLabel={selectedLabel}
        />
      </div>
      <div className="scope-switcher-collapsed">
        <button
          className="nav-item scope-switcher-collapsed__button"
          type="button"
          aria-label={collapsedLabel}
          data-tooltip={collapsedLabel}
          onClick={props.onExpandSidebar}
        >
          <span className="nav-icon" aria-hidden="true">
            {scope.kind === "user" ? <UserIcon /> : <FolderIcon />}
          </span>
        </button>
      </div>
      {/* Out of the sidebar, so its layout can't clip the dialogs. */}
      {createPortal(flow.dialogs, document.body)}
    </>
  );
}
