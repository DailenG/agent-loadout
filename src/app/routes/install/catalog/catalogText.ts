// Display text for catalog data (M45): statuses with their age, publisher
// wording, pinning and kinds. Catalog text (names, descriptions, problems)
// is untrusted data and is rendered as plain text by the callers.

import { onlineManager } from "@tanstack/react-query";
import { storeQueryKeys } from "@/app/apply/changeNotice";
import { errorMessage, failureSentence, isGitNotFound, t, type MessageKey } from "@/i18n/en";
import type {
  CatalogEntry,
  CatalogEntryDetail,
  CatalogEntryKind,
  CatalogInfo,
  CatalogResults,
  IpcError,
} from "@/ipc/bindings";
import { IpcFailure } from "@/ipc/transport";

/** Query keys of the catalog commands; the overview is a store key (store changes refetch it, locally). */
export const catalogQueryKeys = {
  overview: storeQueryKeys.catalogs,
  search: ["search_catalog"],
  /** Prefix of `["show_catalog_entry", catalog, entry]`. */
  show: ["show_catalog_entry"],
} as const;

export const KIND_LABELS: Record<CatalogEntryKind, MessageKey> = {
  server: "catalog.kind.server",
  skill: "catalog.kind.skill",
  plugin: "catalog.kind.plugin",
};

export const CATALOG_KIND_LABELS: Record<CatalogInfo["kind"], MessageKey> = {
  mcp_registry: "catalog.catalogKind.registry",
  git_tap: "catalog.catalogKind.tap",
  link: "catalog.catalogKind.link",
};

/** "5 minutes ago", "3 hours ago", "2 days ago". */
export function formatAge(seconds: number): string {
  if (seconds < 60) {
    return t("catalog.age.now");
  }
  const units: [number, MessageKey, MessageKey][] = [
    [86_400, "catalog.age.day", "catalog.age.days"],
    [3_600, "catalog.age.hour", "catalog.age.hours"],
    [60, "catalog.age.minute", "catalog.age.minutes"],
  ];
  for (const [size, one, many] of units) {
    if (seconds >= size) {
      const count = Math.floor(seconds / size);
      return count === 1 ? t(one) : t(many, { count });
    }
  }
  return t("catalog.age.now");
}

/** "Nov 5, 2026" in the user's locale; the raw value when it does not parse. */
export function formatDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleDateString(undefined, { dateStyle: "medium" });
}

/** "Publisher: io.github.acme (namespace ownership checked by the registry)". */
export function publisherText(entry: CatalogEntry): string | null {
  if (!entry.publisher) {
    return null;
  }
  return t(entry.publisher.checked ? "catalog.publisher.checked" : "catalog.publisher", { name: entry.publisher.name });
}

/** "Pinned to 3f2a9c1", "Follows branch main", "Not pinned". */
export function pinningText(entry: CatalogEntry): string {
  // Server entries come from registries: pinned means every package names an exact version.
  if (entry.kind === "server") {
    return entry.pinned && entry.version
      ? t("catalog.pin.version", { version: entry.version })
      : t("catalog.pin.none");
  }
  const sha = entry.source?.sha ?? (entry.pinned ? entry.commit : null);
  if (entry.pinned && sha) {
    return t("catalog.pin.sha", { sha: sha.slice(0, 7) });
  }
  const followed = entry.source?.gitRef ?? entry.gitRef;
  return followed ? t("catalog.pin.branch", { ref: followed }) : t("catalog.pin.none");
}

/**
 * The ipc error's sentence, with the backend's detail after it. A missing program's detail is diagnostic
 * text ("cannot run git: ..."), so it gets its sentence only: the Git-missing one or the generic one.
 */
export function ipcErrorText(error: IpcError): string {
  if (error.code === "tool_not_found") {
    return failureSentence(error);
  }
  return error.message ? t("catalog.status.errorDetail", { reason: errorMessage(error.code), detail: error.message }) : errorMessage(error.code);
}

/** Git missing for a Git catalog (#29): a neutral notice with a troubleshooting link, not a red error. */
function gitMissingLine(error: IpcError | null): StatusLine | null {
  return error && isGitNotFound(error)
    ? { tone: "neutral", text: t("catalog.status.gitMissing"), help: "git_missing" }
    : null;
}

function isTimeout(error: IpcError | null): boolean {
  return error !== null && /timed? ?out|timeout/i.test(error.message);
}

/** The webview's own online state (its `online`/`offline` events), as TanStack Query tracks it. */
function isOffline(): boolean {
  return !onlineManager.isOnline();
}

/** `help` names a troubleshooting entry the section links to. */
export type StatusLine = { tone: "neutral" | "positive" | "warning" | "danger"; text: string; help?: "git_missing" };

/** One catalog's search status, as its section says it. */
export function statusLine(title: string, results: CatalogResults): StatusLine {
  const count = results.entries.length;
  const found = count === 1 ? t("catalog.results.one") : t("catalog.results.many", { count });
  const age = results.ageSecs === null ? null : formatAge(results.ageSecs);
  switch (results.status) {
    case "fresh":
      return { tone: "positive", text: found };
    case "cached":
      return { tone: "neutral", text: age ? t("catalog.status.cached", { found, age }) : found };
    case "stale":
      return {
        tone: "warning",
        text: isOffline()
          ? t("catalog.status.staleOffline", { found, age: age ?? t("catalog.age.unknown") })
          : isTimeout(results.error)
            ? t("catalog.status.staleTimeout", { found, age: age ?? t("catalog.age.unknown") })
            : t("catalog.status.stale", {
                found,
                age: age ?? t("catalog.age.unknown"),
                error: results.error ? ipcErrorText(results.error) : t("app.error.unknown"),
              }),
      };
    case "off":
      return {
        tone: "warning",
        text: count > 0 && age ? t("catalog.status.off", { found, age }) : t("catalog.status.offEmpty"),
      };
    case "disabled":
      return { tone: "neutral", text: t("catalog.status.disabled") };
    case "query_too_short":
      return { tone: "neutral", text: t("catalog.status.tooShort") };
    case "link":
      return { tone: "neutral", text: t("catalog.status.link") };
    case "error": {
      const gitMissing = isOffline() ? null : gitMissingLine(results.error);
      if (gitMissing) {
        return gitMissing;
      }
      return {
        tone: "danger",
        text: isOffline()
          ? t("catalog.status.offline", { catalog: title })
          : isTimeout(results.error)
            ? t("catalog.status.timedOut", { catalog: title })
            : t("catalog.status.error", {
                catalog: title,
                error: results.error ? ipcErrorText(results.error) : t("app.error.unknown"),
              }),
      };
    }
  }
}

/** A command that failed outright (not a result with status `error`). */
export function failureLine(title: string, failure: unknown): StatusLine {
  if (isOffline()) {
    return { tone: "danger", text: t("catalog.status.offline", { catalog: title }) };
  }
  const error = failure instanceof IpcFailure ? { code: failure.code, message: failure.message } : null;
  const gitMissing = gitMissingLine(error);
  if (gitMissing) {
    return gitMissing;
  }
  if (isTimeout(error)) {
    return { tone: "danger", text: t("catalog.status.timedOut", { catalog: title }) };
  }
  return {
    tone: "danger",
    text: t("catalog.status.error", { catalog: title, error: error ? ipcErrorText(error) : t("app.error.unknown") }),
  };
}

/** How the full entry was answered, when it is not a fresh or in-date one. */
export function detailStatus(title: string, detail: CatalogEntryDetail): StatusLine | null {
  const age = detail.ageSecs === null ? t("catalog.age.unknown") : formatAge(detail.ageSecs);
  switch (detail.status) {
    case "stale":
      return {
        tone: "warning",
        text: t("catalog.detail.stale", {
          age,
          error: detail.error ? ipcErrorText(detail.error) : t("app.error.unknown"),
        }),
      };
    case "off":
      return { tone: "warning", text: t("catalog.detail.off", { age }) };
    case "error":
      return {
        tone: "danger",
        text: isTimeout(detail.error)
          ? t("catalog.status.timedOut", { catalog: title })
          : t("catalog.status.error", {
              catalog: title,
              error: detail.error ? ipcErrorText(detail.error) : t("app.error.unknown"),
            }),
      };
    default:
      return null;
  }
}
