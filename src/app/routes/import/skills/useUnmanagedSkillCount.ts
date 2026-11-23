// The unmanaged skill folders of the user scope, counted for the import
// notice (#9): how many, per client, and a stable key per folder.

import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { USER_ACTIVE_SCOPE } from "@/app/scope/ActiveScope";
import { useTransport } from "@/ipc/transport";
import { countUnmanaged, unmanagedSkillsQuery, type UnmanagedSkillCount } from "./adopt";

export type { UnmanagedSkillCount } from "./adopt";

/** `undefined` while the scan loads, fails, or is turned off (`enabled` false). */
export function useUnmanagedSkillCount(enabled = true): UnmanagedSkillCount | undefined {
  const { commands } = useTransport();
  const scan = useQuery({ ...unmanagedSkillsQuery(commands, USER_ACTIVE_SCOPE, null), enabled });
  return useMemo(() => (enabled && scan.data ? countUnmanaged(scan.data) : undefined), [enabled, scan.data]);
}
