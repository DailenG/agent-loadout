// Dialog shell and one-way password input of the Settings sections
// (SPEC 13.6): the dialog markup of the Projects page, sized to its content.

import { useEffect, useId, useState, type ReactNode } from "react";

export type SettingsDialogProps = {
  title: string;
  subtitle?: string;
  /** Accessible name of the close button. */
  closeLabel: string;
  onClose: () => void;
  /** `alertdialog` for confirmations. */
  role?: "dialog" | "alertdialog";
  wide?: boolean;
  children: ReactNode;
  actions: ReactNode;
};

export function SettingsDialog({
  title,
  subtitle,
  closeLabel,
  onClose,
  role = "dialog",
  wide = false,
  children,
  actions,
}: SettingsDialogProps) {
  const titleId = useId();

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  return (
    <div className="dialog-backdrop" role="presentation" onClick={onClose}>
      <div
        className={`tool-manage-dialog project-dialog settings-dialog${wide ? " settings-dialog--wide" : ""}`}
        role={role}
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="tool-manage-dialog__header">
          <div className="tool-manage-dialog__title">
            <h3 id={titleId}>{title}</h3>
            {subtitle ? <p title={subtitle}>{subtitle}</p> : null}
          </div>
          <button className="tool-manage-dialog__close" type="button" onClick={onClose} aria-label={closeLabel}>
            ×
          </button>
        </div>
        <div className="project-dialog__body settings-dialog__body">{children}</div>
        <div className="tool-manage-dialog__actions">{actions}</div>
      </div>
    </div>
  );
}

export type PasswordDialogProps = {
  title: string;
  subtitle?: string;
  label: string;
  hint?: string;
  submitLabel: string;
  cancelLabel: string;
  /** Receives the value once; the input is cleared before it is sent. */
  onSubmit: (value: string) => Promise<unknown>;
  onClose: () => void;
};

/** A one-way secret input: the value is sent once and never shown again. */
export function PasswordDialog({
  title,
  subtitle,
  label,
  hint,
  submitLabel,
  cancelLabel,
  onSubmit,
  onClose,
}: PasswordDialogProps) {
  const formId = useId();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit() {
    if (value === "") {
      return;
    }
    const sent = value;
    setValue("");
    setBusy(true);
    try {
      await onSubmit(sent);
    } catch {
      // The caller reported the failure.
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsDialog
      title={title}
      subtitle={subtitle}
      closeLabel={cancelLabel}
      onClose={onClose}
      actions={
        <>
          <button className="secondary-button secondary-button--compact" type="button" onClick={onClose}>
            {cancelLabel}
          </button>
          <button
            className="primary-button primary-button--compact"
            type="submit"
            form={formId}
            disabled={busy || value === ""}
          >
            {submitLabel}
          </button>
        </>
      }
    >
      <form
        id={formId}
        className="project-dialog__field"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <label className="project-dialog__field">
          <span>{label}</span>
          <input
            className="tool-manage-dialog__search"
            type="password"
            autoComplete="off"
            value={value}
            onChange={(event) => setValue(event.target.value)}
          />
        </label>
        {hint ? <span className="project-dialog__hint">{hint}</span> : null}
      </form>
    </SettingsDialog>
  );
}
