// Skill commands with what every store change shows (SPEC 8.7, 12.2): the
// queries it makes stale are refetched, and a toast names the change and any
// client folder entry that needs the user. Errors are toasts too, localized
// from their code; callers get the error rethrown to restore their state.

import { useCallback, useMemo } from "react";
import { useInvalidateStore } from "@/app/apply/changeNotice";
import { useNotifications } from "@/app/components/notifications";
import { t } from "@/i18n/en";
import type { SkillChange, SkillInstalled, SkillMethod, SkillScope } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport } from "@/ipc/transport";
import { failureText, syncMessage } from "./labels";

export function useSkillActions() {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const invalidate = useInvalidateStore();

  const run = useCallback(
    async <T,>(
      call: () => Promise<T>,
      done: (value: T) => string | null,
      options: { quietTaken?: boolean; onView?: () => void } = {},
    ): Promise<T> => {
      try {
        const value = await call();
        const message = done(value);
        if (message) {
          notify({
            tone: "success",
            message,
            ...(options.onView ? { actionLabel: t("install.success.view"), onAction: options.onView } : {}),
          });
        }
        return value;
      } catch (error) {
        // A taken id is a question for the user (SPEC 12.1), not a failure.
        if (!(options.quietTaken && error instanceof IpcFailure && error.code === "already_exists")) {
          notify({ tone: "error", message: failureText(error) });
        }
        throw error;
      } finally {
        await invalidate();
      }
    },
    [invalidate, notify],
  );

  const changed = useCallback(
    (change: SkillChange, message: string) => {
      const text = syncMessage(change.links, message);
      if (change.links.needsAttention) {
        notify({ tone: "info", message: text });
        return null;
      }
      return text;
    },
    [notify],
  );

  const installed = useCallback(
    (result: SkillInstalled) => {
      const message = t("install.success", { name: result.name, id: result.id });
      if (result.links?.needsAttention) {
        notify({ tone: "info", message: syncMessage(result.links, message) });
        return null;
      }
      return message;
    },
    [notify],
  );

  return useMemo(
    () => ({
      /** Turns `clients` on or off at user scope (all detected when empty). */
      setClients: (id: string, name: string, clients: string[], enabled: boolean, method: SkillMethod | null = null) =>
        run(
          async () => unwrap(await commands.setSkillClients(id, clients, enabled, method)),
          (change) => changed(change, t(enabled ? "skill.card.success.enabled" : "skill.card.success.disabled", { name })),
        ),
      /** Link or copy; the skill's clients stay as they are (SPEC 12.2). */
      setMethod: (id: string, name: string, method: SkillMethod) =>
        run(
          async () => unwrap(await commands.setSkillMethod(id, method)),
          (change) => changed(change, t("skill.card.success.method", { name })),
        ),
      setProject: (project: string, id: string, name: string, enabled: boolean, hash: string) =>
        run(
          async () => unwrap(await commands.setProjectSkill(project, id, enabled, hash)),
          (change) => changed(change, t(enabled ? "skill.card.success.enabled" : "skill.card.success.disabled", { name })),
        ),
      remove: (id: string, name: string) =>
        run(
          async () => unwrap(await commands.removeSkill(id)),
          (change) => changed(change, t("skill.card.success.deleted", { name })),
        ),
      setTags: (id: string, name: string, tags: string[]) =>
        run(
          async () => unwrap(await commands.setSkillTags(id, tags)),
          () => t("skill.card.tag.saved", { name }),
        ),
      /** Updates to `latest`, the source commit the preview showed. */
      update: (id: string, name: string, latest: string) =>
        run(
          async () => unwrap(await commands.updateSkill(id, latest)),
          (updated) => {
            const message = t("skill.card.success.updated", { name });
            if (updated.links.needsAttention) {
              notify({ tone: "info", message: syncMessage(updated.links, message) });
              return null;
            }
            return message;
          },
        ),
      revealSkill: (id: string) => run(async () => unwrap(await commands.revealSkillFolder(id)), () => null),
      revealTool: (client: string, scope: SkillScope) =>
        run(async () => unwrap(await commands.revealSkillToolFolder(client, scope)), () => null),
      importEntry: (client: string, scope: SkillScope, name: string, renameIfTaken: boolean) =>
        run(
          async () => unwrap(await commands.importToolSkill(client, scope, name, null, renameIfTaken)),
          () => t("skills.source.success.imported", { name }),
          { quietTaken: true },
        ),
      trashEntry: (client: string, scope: SkillScope, name: string) =>
        run(
          async () => unwrap(await commands.trashToolSkill(client, scope, name)),
          () => t("skills.source.success.trashed", { name }),
        ),
      vendor: (name: string, renameIfTaken: boolean) =>
        run(
          async () => unwrap(await commands.vendorAgentsSkill(name, null, renameIfTaken)),
          () => t("skills.agents.vendored", { name }),
          { quietTaken: true },
        ),
      agentsCli: (action: "update" | "remove", name: string) =>
        run(
          async () => unwrap(await commands.runAgentsSkillCli(action, name)),
          () => t("skills.agents.done", { name }),
        ),
      installGit: (
        url: string,
        path: string | null,
        gitRef: string | null,
        id: string | null,
        renameIfTaken: boolean,
        onView?: () => void,
      ) =>
        run(
          async () => unwrap(await commands.installGitSkill(url, path, gitRef, id, renameIfTaken)),
          installed,
          { quietTaken: true, onView },
        ),
      installLocal: (token: string, id: string | null, renameIfTaken: boolean, onView?: () => void) =>
        run(async () => unwrap(await commands.installLocalSkill(token, id, renameIfTaken)), installed, {
          quietTaken: true,
          onView,
        }),
    }),
    [changed, commands, installed, notify, run],
  );
}
