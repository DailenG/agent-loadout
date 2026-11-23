import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { App, createQueryClient } from "@/app/App";
import { t } from "@/i18n/en";
import { createMockTransport } from "@/test/mockTransport";

// "System" names neither the user scope nor its history filter (SPEC 13.1: the
// scope is "User"). Only strings about the operating system keep the word.
const OS_STRINGS = [t("settings.versioning.auth.system"), t("settings.theme.option.system")];

function textWithoutOsStrings() {
  let text = document.body.textContent ?? "";
  for (const allowed of OS_STRINGS) {
    text = text.split(allowed).join("");
  }
  return text;
}

describe("user scope wording", () => {
  it("shows no 'System' for the user scope on any page", async () => {
    const user = userEvent.setup();
    render(<App transport={createMockTransport()} queryClient={createQueryClient()} />);
    const nav = screen.getByRole("navigation", { name: t("app.nav.primary") });
    const labels = within(nav)
      .getAllByRole("button")
      .map((button) => button.textContent ?? "");
    expect(labels.length).toBeGreaterThan(0);
    for (const label of labels) {
      await user.click(within(nav).getByRole("button", { name: label }));
      await waitFor(() => expect(document.body.textContent).toContain(label));
      expect(textWithoutOsStrings(), `page ${label}`).not.toMatch(/\bSystem\b/);
    }

    // History's filter lists User, not System.
    await user.click(within(nav).getByRole("button", { name: "History" }));
    await user.click(await screen.findByRole("combobox", { name: t("history.toolbar.scope") }));
    const options = (await screen.findAllByRole("option")).map((option) => option.textContent);
    expect(options).toContain(t("history.scope.user"));
    expect(textWithoutOsStrings()).not.toMatch(/\bSystem\b/);
  });
});
