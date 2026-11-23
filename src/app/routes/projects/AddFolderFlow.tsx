// Add folder / Init manifest as one flow (SPEC 13.3), shared by the Projects
// page and the sidebar scope switcher: the OS folder picker, then the folder
// dialog, then the trust review of a bound project that isn't trusted yet.
// The picked folder stays in the backend; only its token and placeholder
// path reach the frontend.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";
import { useNotifications } from "@/app/components/notifications";
import { FolderDialog } from "@/app/routes/projects/FolderDialog";
import { TrustReviewDialog } from "@/app/routes/projects/TrustReviewDialog";
import { failureText } from "@/app/routes/projects/labels";
import { projectListQuery } from "@/app/scope/ActiveScope";
import type { PickedFolder, ProjectChange } from "@/ipc/bindings";
import { unwrap, useTransport } from "@/ipc/transport";

export type FolderIntent = "add" | "init";

export type AddFolderFlow = {
  /** Opens the OS folder picker; a cancelled pick ends the flow. */
  start(intent: FolderIntent): void;
  /** Opens the trust review of a listed project. */
  review(projectId: string): void;
  /** True while the OS picker is open. */
  picking: boolean;
  /** The flow's dialogs; render them once. */
  dialogs: ReactNode;
};

export function useAddFolderFlow(options: {
  /** The project exists and is in the project list (trust review may follow). */
  onAdded?: (change: ProjectChange) => void;
} = {}): AddFolderFlow {
  const { commands } = useTransport();
  const { notify } = useNotifications();
  const queryClient = useQueryClient();
  const [picked, setPicked] = useState<{ folder: PickedFolder; intent: FolderIntent } | null>(null);
  const [reviewing, setReviewing] = useState<string | null>(null);

  const pick = useMutation({
    mutationFn: async (intent: FolderIntent) => ({
      folder: unwrap(await commands.pickProjectFolder()),
      intent,
    }),
    onSuccess: ({ folder, intent }) => {
      if (folder) {
        setPicked({ folder, intent });
      }
    },
    onError: (error) => notify({ tone: "error", message: failureText(error) }),
  });

  // Fetched even with no list on screen, so a scope switch to the new
  // project finds it instead of falling back to User.
  async function refreshProjects() {
    try {
      await queryClient.fetchQuery({ ...projectListQuery(commands), staleTime: 0 });
    } catch {
      // The list query shows its own error.
    }
  }

  async function added(change: ProjectChange, review: boolean) {
    setPicked(null);
    await refreshProjects();
    options.onAdded?.(change);
    if (review) {
      setReviewing(change.id);
    }
  }

  const dialogs = (
    <>
      {picked ? (
        <FolderDialog
          folder={picked.folder}
          intent={picked.intent}
          onClose={() => setPicked(null)}
          onDone={(change, review) => void added(change, review)}
        />
      ) : null}
      {reviewing ? (
        <TrustReviewDialog
          projectId={reviewing}
          onClose={() => setReviewing(null)}
          onTrusted={() => {
            setReviewing(null);
            void refreshProjects();
          }}
        />
      ) : null}
    </>
  );

  return {
    start: (intent) => pick.mutate(intent),
    review: setReviewing,
    picking: pick.isPending,
    dialogs,
  };
}
