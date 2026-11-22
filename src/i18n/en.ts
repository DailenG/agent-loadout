// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan
// Shell strings ported from the `en` catalog of DailenG/skilldock@181b52b src/app/i18n.tsx
// (English only, SPEC Decision 13); the rest is new.
//
// The one presentation-layer catalog (SPEC 4.3): display strings keyed by
// message key, IPC enum and error code. Nothing localized crosses IPC; brand
// strings come from the generated bindings (`brand.rs`), never from here.

import {
  BIN_NAME,
  PRODUCT_NAME,
  type DataHomeSource,
  type ErrorCode,
  type Platform,
  type Scope,
} from "@/ipc/bindings";

const messages = {
  "app.nav.primary": "Primary",
  "app.nav.skills.label": "Skills",
  "app.nav.skills.description": "Review installed skills, updates, and pending sync work.",
  "app.nav.mcp.label": "MCP",
  "app.nav.mcp.description": "Scan, edit, and sync MCP config for your AI coding clients.",
  "app.nav.projects.label": "Projects",
  "app.nav.projects.description": "Bind folders to project manifests and review their effective config.",
  "app.nav.tools.label": "Tools",
  "app.nav.tools.description": "Detect supported clients and where their config lives.",
  "app.nav.install.label": "Install",
  "app.nav.install.description": "Bring in new skills and MCP servers from Git repos or local folders.",
  "app.nav.history.label": "History",
  "app.nav.history.description": "Browse every change to your store, compare versions, and roll back.",
  "app.nav.plugins.label": "Plugins",
  "app.nav.plugins.description": "Review managed plugins and the assets they own.",
  "app.nav.publish.label": "Publish",
  "app.nav.publish.description": "Publish managed skills.",
  "app.nav.settings.label": "Settings",
  "app.nav.settings.description": "Appearance, versioning and backup, secrets, and core preferences.",
  "app.nav.about.label": "About",
  "app.nav.about.description": "See version info, repository links, and where your data lives.",
  "app.sidebar.expand": "Expand sidebar",
  "app.sidebar.collapse": "Collapse sidebar",
  "app.window.controls": "Window controls",
  "app.window.minimize": "Minimize",
  "app.window.maximize": "Maximize",
  "app.window.restore": "Restore",
  "app.window.close": "Close",
  "app.action.retry": "Retry",
  "app.error.loadFailed": "Page failed to load",
  "app.error.unknown": "An unknown error occurred",
  "scope.label": "Scope",
  "scope.switcher.aria": "Configuration scope",
  "page.empty.title": "Nothing here yet",
  "page.empty.skills": "Managed skills will be listed here.",
  "page.empty.mcp": "MCP servers and their per-client status will be listed here.",
  "page.empty.projects": "Folders you add as projects will be listed here.",
  "page.empty.tools": "Detected AI coding clients will be listed here.",
  "page.empty.install": "Install sources for skills and MCP servers will appear here.",
  "page.empty.history": "Store commits, applies, and rollbacks will be listed here.",
  "page.empty.plugins": "Plugins are not available in this version.",
  "page.empty.publish": "Publishing is not available in this version.",
  "settings.group.preferences": "App Preferences",
  "settings.theme.label": "Appearance",
  "settings.theme.description": "Choose the interface appearance that best fits your environment.",
  "settings.theme.option.light": "Light",
  "settings.theme.option.dark": "Dark",
  "settings.theme.option.system": "Follow System",
  "about.group.app": "Application",
  "about.version.label": "Version",
  "about.version.description": "{product} {version} on {platform}.",
  "about.identifier.label": "App identifier",
  "about.repository.label": "Repository",
  "about.dataHome.label": "Data directory",
  "about.dataHome.description": "{path} ({source})",
  "about.loading": "Loading...",
  "notifications.close": "Close notification",
  "toolbar.goInstall": "Install",
  "skills.view.aria": "View mode",
  "skills.view.list": "List view",
  "skills.view.grid": "Grid view",
} as const;

export type MessageKey = keyof typeof messages;

export type MessageValues = Record<string, string | number>;

function interpolate(template: string, values?: MessageValues) {
  if (!values) {
    return template;
  }
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in values ? String(values[name]) : match,
  );
}

/** The display string for `key`, with `{name}` placeholders filled. */
export function t(key: MessageKey, values?: MessageValues): string {
  return interpolate(messages[key], values);
}

/** Error and warning codes from the core (SPEC 4.1), one sentence each. */
const errorMessages: Record<ErrorCode, string> = {
  invalid_path: "A path is not usable here.",
  profile_not_found: "Your user profile folder could not be found.",
  io_error: "A file could not be read or written.",
  parse_error: "A file is not valid in its format.",
  unsupported_schema: "A store file was written by a newer version of this app.",
  missing_field: "A required field is missing.",
  invalid_value: "A value is not allowed here.",
  invalid_id: "An identifier does not follow the naming rules.",
  id_mismatch: "An identifier does not match the file that holds it.",
  field_not_allowed: "A field is not allowed for this kind of entry.",
  passthrough_not_allowed: "This client setting cannot be passed through.",
  invalid_ref: "A template or secret reference is not valid.",
  unknown_client: "This client is not known; the entry is kept as written.",
  unknown_os: "This operating system name is not known; the entry is kept as written.",
  locked: `Another ${PRODUCT_NAME} window or command is busy. Try again in a moment.`,
  git_error: "The store repository could not be read or written.",
  already_exists: "An entry with this name already exists.",
  not_found: "The entry was not found.",
  unexpected_shape: "A client file has an unexpected structure.",
  not_supported_on_os: "This client is not available on this operating system.",
  scope_not_supported: "This client has no config for this scope.",
  unsupported_transport: "This client cannot run the server over this transport.",
  unsupported_field: "This client has no setting for one of the server's fields.",
  msix_dual_write: "This client keeps a second copy of its config; both copies are written.",
  collision: "The client treats another entry as the same server.",
  timeout_unit_conflict: "Two clients read this timeout in different units.",
  client_mcp_replaced_by_extension: "An extension takes over this client's MCP support.",
  file_changed_during_apply: "A client file kept changing during apply and was left as it is.",
  self_check_failed: "A written file did not read back as planned and was left as it is.",
  file_locked: "Another program kept a client file open.",
  delivery_unavailable: "The requested secret delivery mode is not available here.",
  no_adapter: "This client is not supported yet; nothing was written for it.",
  needs_secret_extraction: "This entry may hold secrets; extract them before importing.",
  confirmation_required: "This change needs your confirmation.",
  unfinished_apply: "A previous apply did not finish.",
  leak_detected: "A value that looks like a secret was blocked from the store.",
  missing_secret: "A secret has no value.",
  secret_locked: "The secret provider needs you to sign in or approve access.",
  provider_unavailable: "A secret provider is not available on this machine.",
  provider_too_old: "A secret provider is too old for this feature.",
  secret_error: "A secret could not be read.",
  launcher_not_found: `The ${BIN_NAME} launcher could not be found.`,
  no_secret_free_delivery: "This client cannot receive the secret without writing it to its config.",
  secret_in_shared_file: "A secret would be written into a shared project file.",
  inline_env_fallback: "An environment reference is written as its current value.",
  remote_unreachable: "A remote cannot be reached right now; it will be retried later.",
  remote_incomplete: "A remote is still receiving files; it will be retried later.",
  sync_artifact_ignored: "A sync tool's conflict or temporary file was ignored.",
  merge_conflict: "Some changes conflict and need to be resolved.",
  remote_rejected: "The remote refused the push.",
  remote_auth_failed: "The remote refused the credentials.",
  tool_not_found: "A required program is not installed or not on PATH.",
  client_skips_entry: "The client itself skips this entry.",
  unknown_template: "A project entry refers to a server that does not exist.",
  cannot_suppress_in_project: "This client cannot hide a system server in a project.",
  unset_not_expressible: "This client still inherits a value the project unsets.",
  trust_required: "This project needs to be trusted before its config is written.",
  duplicate_via_compat: "The client loads this server twice through another tool's file.",
  duplicate_across_layers: "The client runs two servers with this name.",
  private_in_tracked_file: "A private entry would go into a file tracked by git.",
  project_key_spelling: "The client already knows this folder under another spelling.",
  personal_path_in_shared_file: "A shared project file would contain this machine's folder path.",
  read_only_client: "This client is read only; nothing is written for it.",
  needs_manual_secret: "This value must be set up as a secret by hand.",
  read_only_format: "This file can be read but not safely edited.",
  client_enable_required: "Enable the new server once in the client.",
  restart_required: "Restart the client to load the change.",
};

export function errorMessage(code: ErrorCode): string {
  return errorMessages[code];
}

/** Scope names in the closed switcher. */
export const scopeLabels: Record<Scope["kind"], string> = {
  system: "System",
};

/** Scope names in the open switcher (SPEC 13.1). */
export const scopeOptionLabels: Record<Scope["kind"], string> = {
  system: "System (user-wide)",
};

export const platformLabels: Record<Platform, string> = {
  windows: "Windows",
  linux: "Linux",
  macos: "macOS",
};

export const dataHomeSourceLabels: Record<DataHomeSource, string> = {
  flag: "set by --home",
  env_var: "set by environment variable",
  default: "default location",
};
