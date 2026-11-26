// Catalog keys of the Projects page enums, shared by its dialogs and the
// effective matrix.

import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { LayerKind, ManifestLayerKind, ManifestLayout, ProjectRow, ProjectTrust, ReadOnly } from "@/ipc/bindings";
import { IpcFailure } from "@/ipc/transport";

export const LAYOUT_KEYS = {
  private: "projects.layout.private",
  shared: "projects.layout.shared",
  shared_and_local: "projects.layout.sharedAndLocal",
} as const satisfies Record<ManifestLayout, MessageKey>;

export const TRUST_KEYS = {
  trusted: "projects.trust.trusted",
  untrusted: "projects.trust.untrusted",
  changed: "projects.trust.changed",
  missing: "projects.trust.missing",
  invalid: "projects.trust.invalid",
} as const satisfies Record<ProjectTrust, MessageKey>;

/** The legend letters of the matrix (SPEC 13.3); the core's `system` layer is the user scope. */
export const LAYER_LETTERS: Record<LayerKind, string> = {
  system: "U",
  project: "P",
  project_local: "L",
  foreign: "F",
  client_compat: "C",
};

export const LAYER_KEYS = {
  system: "matrix.layer.user",
  project: "matrix.layer.project",
  project_local: "matrix.layer.projectLocal",
  foreign: "matrix.layer.foreign",
  client_compat: "matrix.layer.clientCompat",
} as const satisfies Record<LayerKind, MessageKey>;

export function failureText(error: unknown) {
  return error instanceof IpcFailure ? errorMessage(error.code) : t("app.error.unknown");
}

/** The prefix the backend puts on paths inside the project folder. */
const PROJECT_ROOT = "<P>/";

/**
 * A path as people read it: `<P>/.mcp.json` (inside the project folder)
 * becomes `.mcp.json`, or `<root>/.mcp.json` when the folder is given.
 * Other paths (user files) are unchanged.
 */
export function projectRelativePath(path: string, root?: string): string {
  if (!path.startsWith(PROJECT_ROOT)) {
    return path;
  }
  const rest = path.slice(PROJECT_ROOT.length);
  return root ? `${root}/${rest}` : rest;
}

/**
 * Any path from IPC as people read it: `<P>/.claude/skills` becomes
 * `.claude/skills in the project folder`, or `<root>/.claude/skills` when
 * the folder is given. Other paths (`~/...`, absolute) are unchanged.
 */
export function displayPath(path: string, root?: string): string {
  if (!path.startsWith(PROJECT_ROOT)) {
    return path;
  }
  return root ? projectRelativePath(path, root) : t("path.inProject", { path: path.slice(PROJECT_ROOT.length) });
}

/** The manifest files as `ProjectRow.readOnlyFiles` names them (M38). */
const MANIFEST_FILES: Record<ManifestLayerKind, string> = {
  shared: `${PROJECT_ROOT}.loadout/project.toml`,
  local: `${PROJECT_ROOT}.loadout/project.local.toml`,
};

/** Why `layer`'s manifest of `project` cannot be written (M38), or null. */
export function manifestReadOnly(project: ProjectRow | undefined | null, layer: ManifestLayerKind): ReadOnly | null {
  if (!project) {
    return null;
  }
  if (project.readOnly) {
    return project.readOnly;
  }
  const entry = project.readOnlyFiles.find((file) => file.file === MANIFEST_FILES[layer]);
  return entry ? { kind: entry.kind } : null;
}
