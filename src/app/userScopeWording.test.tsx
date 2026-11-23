import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { App, createQueryClient } from "@/app/App";
import { t, type MessageKey } from "@/i18n/en";
import { createMockTransport } from "@/test/mockTransport";
import { USER_EVENT_OPTIONS } from "@/test/renderWithProviders";

// "System" names neither the user scope nor its history filter (SPEC 13.1: the
// scope is "User"). Only strings about the operating system keep the word.
const OS_STRINGS = [t("settings.versioning.auth.system"), t("settings.theme.option.system")];

// The sidebar's pages without a feature flag. One test per page: a single
// test that rendered every page in turn ran past the 5s timeout under
// full-suite load.
const PAGES: MessageKey[] = [
  "app.nav.skills.label",
  "app.nav.mcp.label",
  "app.nav.projects.label",
  "app.nav.tools.label",
  "app.nav.install.label",
  "app.nav.history.label",
  "app.nav.settings.label",
  "app.nav.about.label",
];

function textWithoutOsStrings() {
  let text = document.body.textContent ?? "";
  for (const allowed of OS_STRINGS) {
    text = text.split(allowed).join("");
  }
  return text;
}

function renderAt() {
  const user = userEvent.setup(USER_EVENT_OPTIONS);
  render(<App transport={createMockTransport()} queryClient={createQueryClient()} />);
  const nav = screen.getByRole("navigation", { name: t("app.nav.primary") });
  return { user, nav };
}

describe("user scope wording", () => {
  it("lists every page checked below in the sidebar", () => {
    const { nav } = renderAt();
    const labels = within(nav)
      .getAllByRole("button")
      .map((button) => button.textContent ?? "");
    expect(labels).toEqual(PAGES.map((key) => t(key)));
  });

  it.each(PAGES)("shows no 'System' for the user scope on %s", async (key) => {
    const { user, nav } = renderAt();
    const label = t(key);
    await user.click(within(nav).getByRole("button", { name: label }));
    await waitFor(() => expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(label));
    expect(textWithoutOsStrings(), `page ${label}`).not.toMatch(/\bSystem\b/);
  });

  it("lists User, not System, in History's filter", async () => {
    const { user, nav } = renderAt();
    await user.click(within(nav).getByRole("button", { name: t("app.nav.history.label") }));
    await user.click(await screen.findByRole("combobox", { name: t("history.toolbar.scope") }));
    const options = (await screen.findAllByRole("option")).map((option) => option.textContent);
    expect(options).toContain(t("history.scope.user"));
    expect(textWithoutOsStrings()).not.toMatch(/\bSystem\b/);
  });
});
