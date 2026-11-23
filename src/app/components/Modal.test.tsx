import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { Modal } from "@/app/components/Modal";

/** A page with a trigger that opens a dialog, and a live region that stays outside it. */
function Page(props: { dialog: (close: () => void) => ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <div data-testid="shell">
        <h1 tabIndex={-1} data-focus-fallback>
          Page
        </h1>
        <button type="button">Before</button>
        <button type="button" onClick={() => setOpen(true)}>
          Open
        </button>
        <section data-testid="host">
          <button type="button">Sibling</button>
          {open ? props.dialog(() => setOpen(false)) : null}
        </section>
        <button type="button">After</button>
      </div>
      <div data-testid="toasts" data-modal-exempt>
        <button type="button">Toast action</button>
      </div>
    </>
  );
}

function FormDialog({ onClose }: { onClose: () => void }) {
  return (
    <Modal className="test-dialog" labelledBy="form-title" onClose={onClose}>
      <h3 id="form-title">Edit</h3>
      <button type="button" onClick={onClose} aria-label="Close">
        ×
      </button>
      <input aria-label="Name" />
      <button type="button" disabled>
        Disabled
      </button>
      <button type="button" onClick={onClose}>
        Cancel
      </button>
      <button type="button">Save</button>
    </Modal>
  );
}

async function openWithKeyboard(user: UserEvent) {
  const trigger = screen.getByRole("button", { name: "Open" });
  trigger.focus();
  await user.keyboard("{Enter}");
  return trigger;
}

describe("Modal", () => {
  it("moves focus to the first field, labels itself and is modal", async () => {
    const user = userEvent.setup();
    render(<Page dialog={(close) => <FormDialog onClose={close} />} />);
    await openWithKeyboard(user);

    const dialog = screen.getByRole("dialog", { name: "Edit" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveFocus();
  });

  it("focuses a data-autofocus element, else the dialog itself when it has no field", async () => {
    const user = userEvent.setup();
    const { unmount } = render(
      <Page
        dialog={(close) => (
          <Modal className="test-dialog" role="alertdialog" label="Remove?" onClose={close}>
            <input aria-label="Ignored" />
            <button type="button" onClick={close} data-autofocus>
              Keep
            </button>
          </Modal>
        )}
      />,
    );
    await openWithKeyboard(user);
    expect(screen.getByRole("button", { name: "Keep" })).toHaveFocus();
    unmount();

    render(
      <Page
        dialog={(close) => (
          <Modal className="test-dialog" label="Read only" onClose={close}>
            <p>Text</p>
            <button type="button" onClick={close}>
              Close
            </button>
          </Modal>
        )}
      />,
    );
    await openWithKeyboard(user);
    expect(screen.getByRole("dialog", { name: "Read only" })).toHaveFocus();
  });

  it("traps Tab and Shift+Tab inside the dialog", async () => {
    const user = userEvent.setup();
    render(<Page dialog={(close) => <FormDialog onClose={close} />} />);
    await openWithKeyboard(user);
    const dialog = screen.getByRole("dialog", { name: "Edit" });

    const forward: string[] = [];
    for (let press = 0; press < 20; press += 1) {
      await user.tab();
      expect(dialog).toContainElement(document.activeElement as HTMLElement);
      forward.push((document.activeElement as HTMLElement).getAttribute("aria-label") ?? document.activeElement!.textContent!);
    }
    // Name, Cancel, Save, Close, Name, ...: the disabled button is skipped, the last wraps to the first.
    expect(forward.slice(0, 5)).toEqual(["Cancel", "Save", "Close", "Name", "Cancel"]);

    screen.getByRole("button", { name: "Close" }).focus();
    await user.tab({ shift: true });
    expect(screen.getByRole("button", { name: "Save" })).toHaveFocus();
    for (let press = 0; press < 20; press += 1) {
      await user.tab({ shift: true });
      expect(dialog).toContainElement(document.activeElement as HTMLElement);
    }
  });

  it("makes the shell inert while open, except exempt live regions", async () => {
    const user = userEvent.setup();
    render(<Page dialog={(close) => <FormDialog onClose={close} />} />);
    await openWithKeyboard(user);

    const open = screen.getByRole("button", { name: "Open" });
    expect(open).toHaveAttribute("inert");
    expect(screen.getByRole("button", { name: "Sibling" })).toHaveAttribute("inert");
    expect(screen.getByRole("button", { name: "After" })).toHaveAttribute("inert");
    expect(screen.getByTestId("toasts")).not.toHaveAttribute("inert");
    // The dialog's ancestors stay live, or the dialog would be inert too.
    expect(screen.getByTestId("host")).not.toHaveAttribute("inert");
    expect(screen.getByRole("dialog").closest("[inert]")).toBeNull();

    await user.keyboard("{Escape}");
    expect(document.querySelectorAll("[inert]")).toHaveLength(0);
  });

  it("closes on Escape and on the backdrop, and returns focus to the trigger", async () => {
    const user = userEvent.setup();
    render(<Page dialog={(close) => <FormDialog onClose={close} />} />);
    const trigger = await openWithKeyboard(user);
    await user.tab();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(trigger).toHaveFocus();

    await user.keyboard("{Enter}");
    const dialog = screen.getByRole("dialog");
    await user.click(dialog);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    await user.click(dialog.parentElement!);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it("keeps Escape for a control that used it, and stays open while not dismissible", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <Page
        dialog={() => (
          <Modal className="test-dialog" label="Busy" dismissible={false} onClose={onClose}>
            <input aria-label="Field" />
          </Modal>
        )}
      />,
    );
    await openWithKeyboard(user);
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("dialog").parentElement!);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("ignores an Escape a control inside the dialog already handled", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <Page
        dialog={() => (
          <Modal className="test-dialog" label="Menu" onClose={onClose}>
            <input
              aria-label="Combo"
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                }
              }}
            />
          </Modal>
        )}
      />,
    );
    await openWithKeyboard(user);
    await user.keyboard("{Escape}");
    expect(onClose).not.toHaveBeenCalled();
  });

  it("falls back to the page heading when the trigger is gone", async () => {
    const user = userEvent.setup();
    function Removing() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <h1 tabIndex={-1} data-focus-fallback>
            Page
          </h1>
          {open ? null : (
            <button type="button" onClick={() => setOpen(true)}>
              Delete
            </button>
          )}
          {open ? (
            <Modal className="test-dialog" label="Done" onClose={() => setOpen(false)}>
              <button type="button">OK</button>
            </Modal>
          ) : null}
        </>
      );
    }
    render(<Removing />);
    screen.getByRole("button", { name: "Delete" }).focus();
    await user.keyboard("{Enter}");
    // The trigger re-renders as a new element after close; the old one is gone.
    await user.keyboard("{Escape}");
    expect(screen.getByRole("heading", { name: "Page" })).toHaveFocus();
  });

  it("stacks nested dialogs: the top one takes the keyboard and gives it back", async () => {
    const user = userEvent.setup();
    function Outer({ onClose }: { onClose: () => void }) {
      const [confirming, setConfirming] = useState(false);
      return (
        <>
          <Modal className="test-dialog" label="Outer" onClose={onClose}>
            <button type="button" onClick={() => setConfirming(true)}>
              Remove
            </button>
            <button type="button" onClick={onClose}>
              Done
            </button>
          </Modal>
          {confirming ? (
            <Modal className="test-dialog" role="alertdialog" label="Confirm" onClose={() => setConfirming(false)}>
              <button type="button" onClick={() => setConfirming(false)} data-autofocus>
                Cancel
              </button>
              <button type="button">Yes</button>
            </Modal>
          ) : null}
        </>
      );
    }
    render(<Page dialog={(close) => <Outer onClose={close} />} />);
    const trigger = await openWithKeyboard(user);
    const outer = screen.getByRole("dialog", { name: "Outer" });
    const remove = screen.getByRole("button", { name: "Remove" });
    expect(outer).toHaveFocus();
    await user.tab();
    expect(remove).toHaveFocus();

    await user.keyboard("{Enter}");
    const confirm = screen.getByRole("alertdialog", { name: "Confirm" });
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    expect(outer.parentElement).toHaveAttribute("inert");
    for (let press = 0; press < 5; press += 1) {
      await user.tab();
      expect(confirm).toContainElement(document.activeElement as HTMLElement);
    }

    // Escape closes the confirm only; the outer dialog is live again and focus is back on its button.
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(outer).toBeInTheDocument();
    expect(outer.parentElement).not.toHaveAttribute("inert");
    expect(remove).toHaveFocus();
    expect(trigger).toHaveAttribute("inert");

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(trigger).toHaveFocus();
    expect(document.querySelectorAll("[inert]")).toHaveLength(0);
  });

  describe("review follow-up", () => {
    it("falls back to the heading when the dialog and its trigger unmount in the same render", async () => {
      const user = userEvent.setup();
      // "Open on MCP page" from Manage: closing the dialog also replaces the page under it.
      function Navigating() {
        const [page, setPage] = useState<"tools" | "mcp">("tools");
        const [open, setOpen] = useState(false);
        return (
          <>
            <h1 key={page} tabIndex={-1} data-focus-fallback>
              {page}
            </h1>
            {page === "tools" ? (
              <section>
                <button type="button" onClick={() => setOpen(true)}>
                  Manage
                </button>
                {open ? (
                  <Modal className="test-dialog" label="Manage" onClose={() => setOpen(false)}>
                    <button type="button" onClick={() => setPage("mcp")}>
                      Open on MCP page
                    </button>
                  </Modal>
                ) : null}
              </section>
            ) : null}
          </>
        );
      }
      render(<Navigating />);
      screen.getByRole("button", { name: "Manage" }).focus();
      await user.keyboard("{Enter}");
      screen.getByRole("button", { name: "Open on MCP page" }).focus();
      await user.keyboard("{Enter}");
      await waitFor(() => expect(screen.getByRole("heading", { name: "mcp" })).toHaveFocus());
    });

    it("falls back to the heading when a refetch removes the trigger after the dialog closed", async () => {
      const user = userEvent.setup();
      let refetch = () => {};
      function RemoveConfirm() {
        const [rows, setRows] = useState(["app", "lab"]);
        const [removing, setRemoving] = useState<string | null>(null);
        refetch = () => setRows(["lab"]);
        return (
          <>
            <h1 tabIndex={-1} data-focus-fallback>
              Projects
            </h1>
            {rows.map((row) => (
              <button key={row} type="button" onClick={() => setRemoving(row)}>
                Remove {row}
              </button>
            ))}
            {removing ? (
              <Modal className="test-dialog" role="alertdialog" label="Remove?" onClose={() => setRemoving(null)}>
                <button type="button" onClick={() => setRemoving(null)}>
                  Yes
                </button>
              </Modal>
            ) : null}
          </>
        );
      }
      render(<RemoveConfirm />);
      const remove = screen.getByRole("button", { name: "Remove app" });
      remove.focus();
      await user.keyboard("{Enter}");
      screen.getByRole("button", { name: "Yes" }).focus();
      await user.keyboard("{Enter}");
      expect(remove).toHaveFocus();
      // The list refetches after the confirm closed and drops the removed row.
      act(() => refetch());
      expect(remove).not.toBeInTheDocument();
      await waitFor(() => expect(screen.getByRole("heading", { name: "Projects" })).toHaveFocus());
    });

    it("returns focus to the outer dialog's trigger when a confirm and its dialog close together", async () => {
      const user = userEvent.setup();
      function Outer({ onClose }: { onClose: () => void }) {
        const [confirming, setConfirming] = useState(false);
        return (
          <>
            <Modal className="test-dialog" label="Outer" onClose={onClose}>
              <button type="button" onClick={() => setConfirming(true)}>
                Delete
              </button>
            </Modal>
            {confirming ? (
              <Modal className="test-dialog" role="alertdialog" label="Sure?" onClose={() => setConfirming(false)}>
                <button type="button" onClick={onClose}>
                  Yes
                </button>
              </Modal>
            ) : null}
          </>
        );
      }
      render(<Page dialog={(close) => <Outer onClose={close} />} />);
      const trigger = await openWithKeyboard(user);
      await user.tab();
      await user.keyboard("{Enter}");
      screen.getByRole("button", { name: "Yes" }).focus();
      await user.keyboard("{Enter}");
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(screen.queryByRole("alertdialog")).toBeNull();
      await waitFor(() => expect(trigger).toHaveFocus());
    });

    it("keeps the opener as trigger when a child takes focus with autoFocus", async () => {
      const user = userEvent.setup();
      render(
        <Page
          dialog={(close) => (
            <Modal className="test-dialog" label="Paste" onClose={close}>
              <textarea aria-label="Snippet" autoFocus />
            </Modal>
          )}
        />,
      );
      const trigger = await openWithKeyboard(user);
      expect(screen.getByRole("textbox", { name: "Snippet" })).toHaveFocus();
      await user.keyboard("{Escape}");
      expect(trigger).toHaveFocus();
    });

    it("draws a dialog opened over another above it", async () => {
      const user = userEvent.setup();
      const style = document.createElement("style");
      style.textContent = ".card-modal { z-index: 30; } .drawer { z-index: 20; }";
      document.head.append(style);
      try {
        function Detail({ onClose }: { onClose: () => void }) {
          const [drawer, setDrawer] = useState(false);
          return (
            <>
              <Modal className="test-dialog" backdropClassName="card-modal" label="Detail" onClose={onClose}>
                <button type="button" onClick={() => setDrawer(true)}>
                  Pending
                </button>
              </Modal>
              {drawer ? (
                <Modal className="test-dialog" backdropClassName="drawer" label="Drawer" onClose={() => setDrawer(false)}>
                  <button type="button">Apply</button>
                </Modal>
              ) : null}
            </>
          );
        }
        render(<Page dialog={(close) => <Detail onClose={close} />} />);
        await openWithKeyboard(user);
        await user.tab();
        await user.keyboard("{Enter}");
        const detail = screen.getByRole("dialog", { name: "Detail" }).parentElement!;
        const drawer = screen.getByRole("dialog", { name: "Drawer" }).parentElement!;
        expect(Number(getComputedStyle(drawer).zIndex)).toBeGreaterThan(Number(getComputedStyle(detail).zIndex));
      } finally {
        style.remove();
      }
    });

    it("names the question a confirmation asks with aria-describedby", () => {
      render(
        <Modal className="test-dialog" role="alertdialog" label="Remove" describedBy="question" onClose={() => undefined}>
          <p id="question">Remove app from the list?</p>
        </Modal>,
      );
      expect(screen.getByRole("alertdialog", { name: "Remove" })).toHaveAccessibleDescription("Remove app from the list?");
    });
  });
});
