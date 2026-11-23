// Test helper (#11, M29): the keyboard contract every dialog shares, checked
// from the dialog's real trigger. Imported by tests only.

import { screen, waitFor } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";
import { expect } from "vitest";

export type ModalKeyboardCheck = {
  /** The element that opens the dialog with Enter. */
  trigger: HTMLElement;
  /** The dialog's accessible name. */
  name: string;
  role?: "dialog" | "alertdialog";
  /** The question a confirmation asks, exposed through `aria-describedby`. */
  description?: string;
  /** Where focus starts; by default anywhere inside the dialog. */
  initial?: (dialog: HTMLElement) => HTMLElement;
  /** Where focus lands after Escape; by default the trigger. */
  returnTo?: () => HTMLElement;
};

/**
 * Opens the dialog with Enter on its trigger, then checks: focus is inside,
 * 20 Tab and 20 Shift+Tab presses never leave it, Escape closes it and focus
 * returns. Returns nothing; each step asserts.
 */
export async function expectModalKeyboard(user: UserEvent, check: ModalKeyboardCheck) {
  const { trigger, name, role = "dialog" } = check;
  trigger.focus();
  await user.keyboard("{Enter}");
  const dialog = await screen.findByRole(role, { name });
  expect(dialog).toHaveAttribute("aria-modal", "true");
  if (check.description !== undefined) {
    expect(dialog).toHaveAccessibleDescription(check.description);
  }
  if (check.initial) {
    await waitFor(() => expect(check.initial!(dialog)).toHaveFocus());
  } else {
    expect(dialog).toContainElement(document.activeElement as HTMLElement);
  }
  for (const shift of [false, true]) {
    for (let press = 0; press < 20; press += 1) {
      await user.tab({ shift });
      expect(dialog).toContainElement(document.activeElement as HTMLElement);
    }
  }
  await user.keyboard("{Escape}");
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(check.returnTo ? check.returnTo() : trigger).toHaveFocus();
}
