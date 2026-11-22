import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppTooltip } from "@/app/components/AppTooltip";
import { BatchActionBar, BatchDeleteDialog } from "@/app/components/BatchActions";
import {
  ListGridViewToggle,
  type ListGridViewMode,
} from "@/app/components/ListGridViewToggle";
import { NotificationProvider, useNotifications } from "@/app/components/notifications";
import { t } from "@/i18n/en";

afterEach(() => {
  vi.useRealTimers();
});

describe("ported components", () => {
  it("ListGridViewToggle switches between list and grid", async () => {
    function Harness() {
      const [mode, setMode] = useState<ListGridViewMode>("list");
      return <ListGridViewToggle value={mode} onChange={setMode} />;
    }
    const user = userEvent.setup();
    render(<Harness />);

    const grid = screen.getByRole("button", { name: t("skills.view.grid") });
    expect(grid).toHaveAttribute("aria-pressed", "false");
    await user.click(grid);
    expect(grid).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: t("skills.view.list") })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("BatchActionBar runs actions and toggles the selection", async () => {
    const onAction = vi.fn();
    const onToggle = vi.fn();
    const user = userEvent.setup();
    render(
      <BatchActionBar
        actions={[{ key: "remove", label: "Remove", onClick: onAction, tone: "danger" }]}
        ariaLabel="Selection"
        cancelLabel="Cancel"
        deselectAllLabel="Deselect all"
        hint="Select items"
        isAllVisibleSelected={false}
        isBusy={false}
        selectedLabel="2 selected"
        selectAllDisabled={false}
        selectAllLabel="Select all"
        onCancel={() => undefined}
        onToggleSelectAll={onToggle}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Remove" }));
    await user.click(screen.getByRole("button", { name: "Select all" }));
    expect(onAction).toHaveBeenCalledOnce();
    expect(onToggle).toHaveBeenCalledOnce();
    expect(screen.getByText("2 selected")).toBeInTheDocument();
  });

  it("BatchDeleteDialog cancels on Escape", async () => {
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <BatchDeleteDialog
        cancelLabel="Cancel"
        confirmLabel="Delete"
        description="This cannot be undone."
        isBusy={false}
        isOpen
        title="Delete 2 items?"
        onCancel={onCancel}
        onConfirm={() => undefined}
      />,
    );

    expect(screen.getByRole("dialog", { name: "Delete 2 items?" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("AppTooltip shows data-tooltip text after the hover delay", () => {
    vi.useFakeTimers();
    render(
      <>
        <button type="button" data-tooltip="Refresh the list">
          Refresh
        </button>
        <AppTooltip />
      </>,
    );

    act(() => {
      screen
        .getByRole("button", { name: "Refresh" })
        .dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
    });
    expect(screen.queryByRole("tooltip")).toBeNull();
    act(() => {
      vi.advanceTimersByTime(600);
    });
    expect(screen.getByRole("tooltip")).toHaveTextContent("Refresh the list");
  });

  it("notifications show a toast and close it", async () => {
    function Notifier() {
      const { notify } = useNotifications();
      return (
        <button type="button" onClick={() => notify({ message: "Saved", tone: "success" })}>
          Save
        </button>
      );
    }
    const user = userEvent.setup();
    render(
      <NotificationProvider>
        <Notifier />
      </NotificationProvider>,
    );

    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(screen.getByRole("status")).toHaveTextContent("Saved");
    await user.click(screen.getByRole("button", { name: t("notifications.close") }));
    expect(screen.getByRole("status")).toHaveClass("is-exiting");
  });
});
