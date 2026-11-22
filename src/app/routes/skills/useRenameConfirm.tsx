// SPEC 12.1: a skill whose id is taken is installed as `<id>-2` only after
// confirmation. `confirmRename()` opens the dialog and resolves to the answer.

import { useCallback, useRef, useState, type ReactNode } from "react";
import { BatchDeleteDialog } from "@/app/components/BatchActions";
import { t } from "@/i18n/en";

export function useRenameConfirm(): { confirmRename: () => Promise<boolean>; dialog: ReactNode } {
  const [open, setOpen] = useState(false);
  const resolveRef = useRef<((answer: boolean) => void) | null>(null);

  const confirmRename = useCallback(
    () =>
      new Promise<boolean>((resolve) => {
        resolveRef.current = resolve;
        setOpen(true);
      }),
    [],
  );

  const answer = useCallback((value: boolean) => {
    setOpen(false);
    resolveRef.current?.(value);
    resolveRef.current = null;
  }, []);

  const dialog = (
    <BatchDeleteDialog
      isOpen={open}
      isBusy={false}
      title={t("skills.rename.title")}
      description={t("skills.rename.description")}
      confirmLabel={t("skills.rename.confirm")}
      cancelLabel={t("skills.rename.cancel")}
      onCancel={() => answer(false)}
      onConfirm={() => answer(true)}
    />
  );

  return { confirmRename, dialog };
}
