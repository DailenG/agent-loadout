// The adopt flow in a dialog, opened from a client's skill folder on the
// Skills page: one folder ("Adopt") or every name with one content ("Adopt
// all identical").

import { useId } from "react";
import { Modal } from "@/app/components/Modal";
import { t } from "@/i18n/en";
import type { SkillScope } from "@/ipc/bindings";
import { AdoptPanel } from "./AdoptPanel";

export type AdoptDialogProps = {
  title: string;
  scope: SkillScope;
  client: string;
  /** One folder's name; without it, every name with one content. */
  name?: string;
  onClose: () => void;
};

export function AdoptDialog({ title, scope, client, name, onClose }: AdoptDialogProps) {
  const titleId = useId();
  const introId = useId();
  return (
    <Modal
      className="tool-manage-dialog project-dialog adopt-dialog"
      labelledBy={titleId}
      describedBy={introId}
      onClose={onClose}
    >
      <div className="tool-manage-dialog__header">
        <div className="tool-manage-dialog__title">
          <h3 id={titleId}>{title}</h3>
          <p id={introId}>{t("adopt.dialog.intro")}</p>
        </div>
        <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={t("adopt.dialog.close")}>
          ×
        </button>
      </div>
      <div className="adopt-dialog__body">
        <AdoptPanel
          scope={scope}
          client={client}
          names={name ? [name] : undefined}
          identicalOnly={!name}
          onCancel={onClose}
        />
      </div>
    </Modal>
  );
}
