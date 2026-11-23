// Catalog keys of the Projects page enums, shared by its dialogs and the
// effective matrix.

import { errorMessage, t, type MessageKey } from "@/i18n/en";
import type { LayerKind, ManifestLayout, ProjectTrust } from "@/ipc/bindings";
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
