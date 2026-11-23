// Skills adopt (#3, SPEC 12.5): the scan of a scope's unmanaged skill
// folders, the choices the user makes per name, and the counts the import
// notice reads. The scan is an import query: adopts and discoveries refetch
// it (`useInvalidateImports`), drift when it is older than a minute.

import { importQueryKeys } from "@/app/apply/changeNotice";
import type {
  SkillAdoptAction,
  SkillAdoptChoice,
  SkillScope,
  UnmanagedSkill,
  UnmanagedSkillVariant,
  UnmanagedSkills,
} from "@/ipc/bindings";
import { unwrap, type Commands } from "@/ipc/transport";

/**
 * `scan_unmanaged_skills` for one scope and one client (`null`: every client).
 * Its failure is shown by the panel, and the notice's count just stays away,
 * so no error toast repeats on every trigger.
 */
export function unmanagedSkillsQuery(commands: Commands, scope: SkillScope, client: string | null) {
  return {
    queryKey: [...importQueryKeys.unmanagedSkills, scope, client],
    queryFn: async () => unwrap(await commands.scanUnmanagedSkills(scope, client)),
    meta: { inlineError: true },
  };
}

/** What the user chose for one name: one content, every content, or nothing. */
export type AdoptMode = "pick" | "both" | "skip";

export type AdoptDecision = {
  mode: AdoptMode;
  /** The content that gets the name (a variant hash). */
  variant: string;
};

/**
 * The content that has to keep the name: the store copy when the store holds
 * this name (an adopt never overwrites it), else `null` (the user picks).
 */
export function fixedVariant(candidate: UnmanagedSkill): UnmanagedSkillVariant | null {
  return candidate.variants.find((variant) => variant.inStore) ?? null;
}

/**
 * The preselected decision: one content is adopted as is; several wait
 * for the user (skip), with the store copy, else the content most clients
 * hold, as the one that keeps the name.
 */
export function initialDecision(candidate: UnmanagedSkill): AdoptDecision {
  const fixed = fixedVariant(candidate);
  const widest = [...candidate.variants].sort((left, right) => right.folders.length - left.folders.length)[0];
  const variant = (fixed ?? widest)?.hash ?? "";
  return { mode: candidate.conflict ? "skip" : "pick", variant };
}

/**
 * One explicit choice per scanned candidate: the `offered` names as decided,
 * every other name skipped. The core reports a scanned conflict without a
 * choice as a conflict (needs attention), so a name the user never saw
 * (one folder's Adopt, "Adopt all identical") must not be left out.
 */
export function adoptChoices(
  scanned: readonly UnmanagedSkill[],
  offered: ReadonlySet<string>,
  decisions: Readonly<Record<string, AdoptDecision>>,
): SkillAdoptChoice[] {
  return scanned.map((candidate) => {
    const { mode, variant } = offered.has(candidate.name)
      ? (decisions[candidate.name] ?? initialDecision(candidate))
      : { mode: "skip" as const, variant: "" };
    const action: SkillAdoptAction =
      mode === "pick" ? { kind: "pick", variant } : mode === "both" ? { kind: "both", primary: variant } : { kind: "skip" };
    return { name: candidate.name, action };
  });
}

/**
 * The id a content other than the chosen one gets with "keep both":
 * `name-<client>`, the first client that holds it (the backend adds `-2`
 * when that id is taken by other content).
 */
export function renamedId(name: string, variant: UnmanagedSkillVariant): string | null {
  const first = variant.folders[0];
  return first ? `${name}-${first.client}` : null;
}

export type UnmanagedSkillCount = {
  /** Unmanaged skill folders that can be adopted, across clients. */
  count: number;
  /** The same, per client id. */
  byClient: Record<string, number>;
  /**
   * One stable key per folder (`<client>:<name>:<content hash>`), so a
   * notice can remember what it showed and not repeat it.
   */
  keys: string[];
};

export function countUnmanaged(scan: UnmanagedSkills): UnmanagedSkillCount {
  const byClient: Record<string, number> = {};
  const keys: string[] = [];
  for (const candidate of scan.candidates) {
    for (const variant of candidate.variants) {
      for (const folder of variant.folders) {
        byClient[folder.client] = (byClient[folder.client] ?? 0) + 1;
        keys.push(`${folder.client}:${candidate.name}:${variant.hash}`);
      }
    }
  }
  return { count: keys.length, byClient, keys };
}
