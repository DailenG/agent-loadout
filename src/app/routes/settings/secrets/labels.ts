// Catalog keys of Settings > Secrets (SPEC 11.3, 11.11, 13.6 item 5),
// shared by the checklist and its dialogs.

import { t, type MessageKey } from "@/i18n/en";
import type {
  AliasCheck,
  NeededBy,
  OpFeatureDto,
  ProviderKindDto,
  ProviderStateDto,
  SecretStatusKind,
} from "@/ipc/bindings";

export const STATUS_KEYS = {
  resolved: "settings.secrets.status.resolved",
  missing: "settings.secrets.status.missing",
  locked: "settings.secrets.status.locked",
  provider_unavailable: "settings.secrets.status.providerUnavailable",
  provider_too_old: "settings.secrets.status.providerTooOld",
  unverifiable: "settings.secrets.status.unverifiable",
  error: "settings.secrets.status.error",
} as const satisfies Record<SecretStatusKind, MessageKey>;

export const STATUS_TONES: Record<SecretStatusKind, string> = {
  resolved: "tone-positive",
  missing: "tone-danger",
  locked: "tone-warning",
  provider_unavailable: "tone-danger",
  provider_too_old: "tone-warning",
  unverifiable: "tone-info",
  error: "tone-danger",
};

export const PROVIDER_KEYS = {
  env: "settings.secrets.provider.env",
  keyring: "settings.secrets.provider.keyring",
  onepassword: "settings.secrets.provider.onepassword",
} as const satisfies Record<ProviderKindDto, MessageKey>;

export const PROVIDER_STATE_KEYS = {
  available: "settings.secrets.providerState.available",
  unavailable: "settings.secrets.providerState.unavailable",
  error: "settings.secrets.providerState.error",
} as const satisfies Record<ProviderStateDto, MessageKey>;

export const PROVIDER_STATE_TONES: Record<ProviderStateDto, string> = {
  available: "tone-positive",
  unavailable: "tone-warning",
  error: "tone-danger",
};

export const FEATURE_KEYS = {
  item_read: "settings.secrets.op.feature.itemRead",
  environments: "settings.secrets.op.feature.environments",
  environment_listing: "settings.secrets.op.feature.environmentListing",
} as const satisfies Record<OpFeatureDto, MessageKey>;

/** A 1Password alias without the CLI reads "1Password CLI not found" (SPEC 11.11). */
export function statusLabel(check: AliasCheck) {
  const reference = check.reference ?? "";
  if (check.status === "provider_unavailable" && (reference.startsWith("op://") || reference.startsWith("op-env://"))) {
    return t("settings.secrets.op.notFound");
  }
  return t(STATUS_KEYS[check.status]);
}

/** "needed by github (claude-code, codex); fetch (not enabled)". */
export function neededByText(neededBy: NeededBy[]) {
  if (neededBy.length === 0) {
    return t("settings.secrets.needed.none");
  }
  const servers = neededBy.map((need) => {
    const clients =
      need.clients.length === 0
        ? t("settings.secrets.needed.notEnabled")
        : need.clients
            .map((client) => (client === "all-detected" ? t("settings.secrets.needed.allDetected") : client))
            .join(", ");
    return t("settings.secrets.needed.server", { server: need.server, clients });
  });
  return t("settings.secrets.needed.by", { servers: servers.join("; ") });
}
