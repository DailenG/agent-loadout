// The skill install review flow (M40, M42), shared by Install > Skill (Git,
// Local) and Install > Browse (M45): fetch and scan, show the review, then
// install exactly that content.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTrustPrompt } from "@/app/components/TrustDialog";
import { useNotifications } from "@/app/components/notifications";
import { errorMessage } from "@/i18n/en";
import type { SkillInstallReview } from "@/ipc/bindings";
import { IpcFailure, unwrap, useTransport, type CommandResult } from "@/ipc/transport";
import { useSkillActions } from "../skills/actions";
import { failureText } from "../skills/labels";
import { useRenameConfirm } from "../skills/useRenameConfirm";
import { SkillReviewDialog } from "./SkillReviewDialog";

export type ReviewLoader = () => Promise<CommandResult<SkillInstallReview>>;

type OpenReview = {
  review: SkillInstallReview;
  id: string | null;
  onDone: () => void;
  /** Where the review came from, shown in the dialog (a catalog entry, M45). */
  origin: ReactNode;
};

/**
 * Install through the review (M40, M42): fetch and scan, show the review,
 * then install exactly that content. Install asks the trust question first
 * when the source needs it; a taken id asks, then installs as `<id>-2`
 * (SPEC 12.1). Closing the review without installing discards it.
 */
export function useReviewInstall(onView: () => void) {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const actions = useSkillActions();
  const { confirmRename, dialog: renameDialog } = useRenameConfirm();
  const { resolveTrust, dialog: trustDialog } = useTrustPrompt();
  const [open, setOpen] = useState<OpenReview | null>(null);
  const [isInstalling, setIsInstalling] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  /**
   * Fetches and scans; resolves once the review is shown (or failed, with a toast). A review that
   * finishes after the page went away is discarded, so its temporary copy is removed.
   */
  async function review(load: ReviewLoader, id: string | null, onDone: () => void, origin: ReactNode = null) {
    try {
      const loaded = unwrap(await load());
      if (!mounted.current) {
        void commands.discardSkillReview(loaded.token);
        return;
      }
      setOpen({ review: loaded, id, onDone, origin });
    } catch (error) {
      if (mounted.current) {
        notify({ tone: "error", message: failureText(error) });
      }
    }
  }

  function close() {
    // An install in flight completes in the backend: closing now would discard a review that still installs.
    if (isInstalling) {
      return;
    }
    if (open) {
      void commands.discardSkillReview(open.review.token);
    }
    setOpen(null);
  }

  async function install(allowDangerous: boolean) {
    if (!open) {
      return;
    }
    const { review: current, id, onDone } = open;
    const answer = await resolveTrust(current, () =>
      notify({ tone: "error", message: errorMessage("source_blocked") }),
    );
    if (!answer) {
      return;
    }
    setIsInstalling(true);
    const run = (renameIfTaken: boolean) =>
      actions.installReviewed(current.token, id, renameIfTaken, allowDangerous, answer, onView);
    try {
      try {
        await run(false);
      } catch (error) {
        if (!(error instanceof IpcFailure) || error.code !== "already_exists" || !(await confirmRename())) {
          return;
        }
        await run(true);
      }
      // Installed: the backend consumed the review.
      setOpen(null);
      onDone();
    } catch {
      // Reported by the action; the review stays for a retry.
    } finally {
      setIsInstalling(false);
    }
  }

  const dialog = (
    <>
      {open ? (
        <SkillReviewDialog
          review={open.review}
          origin={open.origin}
          isInstalling={isInstalling}
          onInstall={(allowDangerous) => void install(allowDangerous)}
          onClose={close}
        />
      ) : null}
      {trustDialog}
      {renameDialog}
    </>
  );

  return { review, dialog };
}
