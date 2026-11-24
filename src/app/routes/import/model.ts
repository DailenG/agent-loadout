// MCP import (#3, SPEC 8.6): what the user chose per scanned candidate, the
// checks on it, and the `apply_import` request built from it. Pure, so the
// import section, its review and the tests share one definition.

import type {
  ImportCandidate,
  ImportChoices,
  ImportPick,
  ImportScope,
  ImportSecretChoice,
  ImportSecretTarget,
  ImportSource,
  ImportVariant,
  ManifestLayerKind,
  ProjectRow,
} from "@/ipc/bindings";
import type { ActiveScope } from "@/app/scope/ActiveScope";
import type { MessageKey } from "@/i18n/en";

/** Where an "Import…" button points: a candidate, and the finding that was clicked. */
export type ImportTarget = {
  name: string;
  /** The finding id (`<source key>/<name>`): its definition is the one picked. */
  findingId?: string;
};

/** How one candidate is imported: one definition, every definition, or not at all. */
export type ImportMode = "pick" | "both" | "skip";

export type CandidateChoice = {
  mode: ImportMode;
  /** The picked definition (`ImportVariant.key`) for `pick`. */
  variant: string;
};

/** A secret's destination as typed: `text` is the 1Password reference or the variable name. */
export type SecretAnswer = {
  choice: "keyring" | "onepassword" | "env" | "skip";
  text: string;
};

export const DEFAULT_SECRET_ANSWER: SecretAnswer = { choice: "keyring", text: "" };

/** A store server id (`model::ids`, slug rule). */
const SERVER_ID = /^[a-z0-9][a-z0-9._-]{0,63}$/;
/**
 * `op://vault/item/field` or `op://vault/item/section/field` (SPEC 11.2): each part non-blank,
 * spaces allowed inside (`op://Private/Brave Search/credential`), as core checks it.
 */
const OP_REFERENCE = /^op:\/\/[^/]*\S[^/]*\/[^/]*\S[^/]*\/[^/]*\S[^/]*(\/[^/]*\S[^/]*)?$/;
/** A 1Password Environment variable (`op-env://<environment-id>/<NAME>`), as core parses it. */
const OP_ENV_REFERENCE = /^op-env:\/\/[a-z0-9]+\/[A-Za-z_][A-Za-z0-9_]*$/;
/** An environment variable name. */
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** The scan's scope for the sidebar scope: the import view always scans everything and filters. */
export const SCAN_ALL: ImportScope = { kind: "all" };

/** Query key of the scan; not a drift key, so a window focus never reshuffles open choices. */
export const IMPORT_SCAN_KEY = ["scan_import", "all"] as const;

/** Names for client and project ids, and what the review needs to know about a client. */
export type ImportNames = {
  client: (id: string) => string;
  project: (id: string | null) => ProjectRow | undefined;
  /** The client has a private project target (`ToolInfo.privateProjectTarget`). */
  privateTarget: (client: string) => boolean;
};

/** The unmanaged summary's scope for the sidebar scope: user files, or one project's files. */
export function importScopeOf(scope: ActiveScope): ImportScope {
  return scope.kind === "project" ? { kind: "project", id: scope.id } : { kind: "user" };
}

export function importableVariants(candidate: ImportCandidate): ImportVariant[] {
  return candidate.variants.filter((variant) => variant.draft !== null && variant.error === null);
}

/** The variant one of the candidate's findings belongs to. */
function variantOfFinding(candidate: ImportCandidate, findingId: string | undefined): ImportVariant | undefined {
  if (!findingId) {
    return undefined;
  }
  return importableVariants(candidate).find((variant) => variant.sources.some((source) => source.id === findingId));
}

/**
 * What a candidate starts with. A target picks its candidate (the clicked
 * finding's definition) and nothing else. Otherwise a single definition not
 * in the store is picked, and a conflict or a name the store has waits for
 * the user.
 */
export function defaultChoice(candidate: ImportCandidate, target: ImportTarget | null): CandidateChoice {
  const importable = importableVariants(candidate);
  const first = importable[0];
  if (!first) {
    return { mode: "skip", variant: "" };
  }
  if (target) {
    if (target.name !== candidate.name) {
      return { mode: "skip", variant: first.key };
    }
    return { mode: "pick", variant: (variantOfFinding(candidate, target.findingId) ?? first).key };
  }
  if (candidate.conflict || candidate.inStore) {
    return { mode: "skip", variant: first.key };
  }
  return { mode: "pick", variant: first.key };
}

/** The id a definition gets unless the user edits it: lowercase, as core suggests it. */
export function defaultId(candidate: ImportCandidate, variant: ImportVariant, mode: ImportMode): string {
  return mode === "pick" && !candidate.inStore ? candidate.name.toLowerCase() : variant.suggestedId;
}

/** The definitions a choice imports. */
export function chosenVariants(candidate: ImportCandidate, choice: CandidateChoice): ImportVariant[] {
  const importable = importableVariants(candidate);
  if (choice.mode === "skip") {
    return [];
  }
  if (choice.mode === "both") {
    return importable;
  }
  const picked = importable.find((variant) => variant.key === choice.variant);
  return picked ? [picked] : [];
}

export function editKey(name: string, part: string): string {
  return `${name}\u0000${part}`;
}

/** Why `answer` cannot be used, or null. */
export function secretProblem(answer: SecretAnswer): MessageKey | null {
  const text = answer.text.trim();
  switch (answer.choice) {
    case "keyring":
    case "skip":
      return null;
    case "onepassword":
      if (text === "") {
        return "import.secret.missing";
      }
      return OP_REFERENCE.test(text) || OP_ENV_REFERENCE.test(text) ? null : "import.secret.badOp";
    case "env":
      if (text === "") {
        return "import.secret.missing";
      }
      return ENV_NAME.test(text) ? null : "import.secret.badEnv";
  }
}

export function secretTarget(answer: SecretAnswer): ImportSecretTarget | null {
  switch (answer.choice) {
    case "keyring":
      return { kind: "keyring" };
    case "onepassword":
      return { kind: "onepassword", reference: answer.text.trim() };
    case "env":
      return { kind: "env", name: answer.text.trim() };
    case "skip":
      return null;
  }
}

/** Why `id` cannot be a new store server's id, or null. */
export function idProblem(id: string, storeIds: ReadonlySet<string>, usedTwice: boolean): MessageKey | null {
  if (!SERVER_ID.test(id)) {
    return "import.id.invalid";
  }
  if (storeIds.has(id)) {
    return "import.id.taken";
  }
  return usedTwice ? "import.id.duplicate" : null;
}

/** The secret fields of the chosen definitions, once each, in order. */
export function chosenSecretFields(variants: ImportVariant[]): string[] {
  const fields: string[] = [];
  for (const variant of variants) {
    for (const secret of variant.secrets) {
      if (!fields.includes(secret.field)) {
        fields.push(secret.field);
      }
    }
  }
  return fields;
}

/**
 * Secret fields more than one chosen definition holds. Their destination is
 * keyed by candidate and field, so one 1Password reference or variable would
 * stand for two different values: only the keyring (one alias per
 * definition, each value read from its own file) or skipping fits.
 */
export function sharedSecretFields(variants: ImportVariant[]): Set<string> {
  const seen = new Set<string>();
  const shared = new Set<string>();
  for (const variant of variants) {
    for (const secret of variant.secrets) {
      if (seen.has(secret.field)) {
        shared.add(secret.field);
      }
      seen.add(secret.field);
    }
  }
  return shared;
}

/** One definition the request imports, for the review. */
export type PlannedPick = {
  candidate: ImportCandidate;
  variant: ImportVariant;
  id: string;
  /** The secret fields of this definition and where each one goes. */
  secrets: { field: string; answer: SecretAnswer }[];
};

export type ImportPlan = {
  request: ImportChoices;
  picks: PlannedPick[];
  /** Candidates skipped because a secret's destination is "Skip this server". */
  skippedForSecrets: string[];
  /** Per candidate (name), the problems that block the review. */
  problems: Map<string, MessageKey[]>;
  /** Per edit key (name + variant key), the id problem. */
  idProblems: Map<string, MessageKey>;
};

export type ImportState = {
  choices: Record<string, CandidateChoice>;
  /** Edited ids by `editKey(name, variant key)`. */
  ids: Record<string, string>;
  /** Secret answers by `editKey(name, field)`. */
  secrets: Record<string, SecretAnswer>;
};

export function choiceOf(state: ImportState, candidate: ImportCandidate, target: ImportTarget | null): CandidateChoice {
  return state.choices[candidate.name] ?? defaultChoice(candidate, target);
}

export function idOf(state: ImportState, candidate: ImportCandidate, variant: ImportVariant, mode: ImportMode): string {
  return state.ids[editKey(candidate.name, variant.key)] ?? defaultId(candidate, variant, mode);
}

/** The answer for a field; a shared field (`sharedSecretFields`) falls back to the keyring for a reference. */
export function answerOf(state: ImportState, name: string, field: string, shared = false): SecretAnswer {
  const answer = state.secrets[editKey(name, field)] ?? DEFAULT_SECRET_ANSWER;
  return shared && (answer.choice === "onepassword" || answer.choice === "env") ? DEFAULT_SECRET_ANSWER : answer;
}

/** Builds the request and its checks from what the user chose. */
export function planImport(
  candidates: ImportCandidate[],
  state: ImportState,
  target: ImportTarget | null,
  storeIds: ReadonlySet<string>,
): ImportPlan {
  const picks: PlannedPick[] = [];
  const secrets: ImportSecretChoice[] = [];
  const skippedForSecrets: string[] = [];
  const problems = new Map<string, MessageKey[]>();
  const addProblem = (name: string, problem: MessageKey) =>
    problems.set(name, [...(problems.get(name) ?? []), problem]);

  for (const candidate of candidates) {
    const choice = choiceOf(state, candidate, target);
    const variants = chosenVariants(candidate, choice);
    if (variants.length === 0) {
      continue;
    }
    const fields = chosenSecretFields(variants);
    const shared = sharedSecretFields(variants);
    const answerFor = (field: string) => answerOf(state, candidate.name, field, shared.has(field));
    const skipFields = new Set(fields.filter((field) => answerFor(field).choice === "skip"));
    // "Skip this server" for a field drops every chosen definition holding it.
    const kept = variants.filter((variant) => !variant.secrets.some((secret) => skipFields.has(secret.field)));
    if (kept.length < variants.length) {
      skippedForSecrets.push(candidate.name);
    }
    for (const field of fields) {
      const answer = answerFor(field);
      const problem = secretProblem(answer);
      if (problem) {
        addProblem(candidate.name, problem);
      }
      const target = secretTarget(answer);
      if (target && kept.some((variant) => variant.secrets.some((secret) => secret.field === field))) {
        secrets.push({ name: candidate.name, field, target });
      }
    }
    for (const variant of kept) {
      picks.push({
        candidate,
        variant,
        id: idOf(state, candidate, variant, choice.mode).trim(),
        secrets: variant.secrets.map((secret) => ({
          field: secret.field,
          answer: answerFor(secret.field),
        })),
      });
    }
  }

  const idProblems = new Map<string, MessageKey>();
  const counts = new Map<string, number>();
  for (const pick of picks) {
    counts.set(pick.id, (counts.get(pick.id) ?? 0) + 1);
  }
  for (const pick of picks) {
    const problem = idProblem(pick.id, storeIds, (counts.get(pick.id) ?? 0) > 1);
    if (problem) {
      idProblems.set(editKey(pick.candidate.name, pick.variant.key), problem);
      addProblem(pick.candidate.name, problem);
    }
  }

  const request: ImportChoices = {
    picks: picks.map<ImportPick>((pick) => ({ name: pick.candidate.name, source: pick.variant.key, id: pick.id })),
    secrets,
    only: false,
  };
  return { request, picks, skippedForSecrets, problems, idProblems };
}

/** The user-file clients a definition is enabled for (none: a definition only). */
export function enabledClients(variant: ImportVariant): string[] {
  const clients: string[] = [];
  for (const source of variant.sources) {
    if (source.scope === "user" && !clients.includes(source.client)) {
      clients.push(source.client);
    }
  }
  return clients;
}

/** The known projects a definition was found in. */
export function projectsOf(variant: ImportVariant): ImportSource[] {
  const seen = new Set<string>();
  return variant.sources.filter((source) => {
    if (!source.projectId || seen.has(source.projectId)) {
      return false;
    }
    seen.add(source.projectId);
    return true;
  });
}

/** Where "Add to project manifest" writes a definition found in one project's files. */
export type ManifestPlacement = {
  /** The clients whose files hold it there: the entry is enabled for them only. */
  clients: string[];
  layer: ManifestLayerKind;
  /**
   * The entry sits in the shared file of a client with a private target:
   * the user chooses between managing that shared entry (shared manifest)
   * and adding a private one next to it.
   */
  offersChoice: boolean;
  /** The project has a shared manifest (`project.toml`) to write to. */
  sharedAvailable: boolean;
};

/**
 * `hasPrivateTarget(client)`: the client has a private project target next to
 * its shared file (`ToolInfo.privateProjectTarget`, from the adapter table),
 * where a private manifest entry is written instead of over the shared entry.
 */
export function manifestPlacement(
  variant: ImportVariant,
  projectId: string,
  project: ProjectRow | undefined,
  hasPrivateTarget: (client: string) => boolean,
): ManifestPlacement {
  const sources = variant.sources.filter((source) => source.projectId === projectId);
  const clients = [...new Set(sources.map((source) => source.client))];
  const inShared = sources.filter((source) => source.scope === "project");
  const sharedAvailable = project?.layout === "shared" || project?.layout === "shared_and_local";
  const offersChoice = inShared.some((source) => hasPrivateTarget(source.client));
  // An entry in a shared file is managed through the shared manifest (the user asked for that entry).
  const layer: ManifestLayerKind = inShared.length > 0 && sharedAvailable ? "shared" : "local";
  return { clients, layer, offersChoice, sharedAvailable };
}

export type SourceFilter = "all" | "user" | "projects";

export function matchesFilter(candidate: ImportCandidate, filter: SourceFilter): boolean {
  if (filter === "all") {
    return true;
  }
  return candidate.variants.some((variant) =>
    variant.sources.some((source) => (filter === "user" ? source.scope === "user" : source.scope !== "user")),
  );
}
