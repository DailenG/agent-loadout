import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeIntersectionObserver as Fake } from "@/test/fakeIntersectionObserver";
import { useVirtualList, VirtualSlot } from "./useVirtualSlots";

const ITEMS = Array.from({ length: 100 }, (_, index) => `item-${index}`);
const indexOf = (target: HTMLElement) => Number(target.dataset.index);

/** Content renders per item, counted. */
const renders = new Map<string, number>();
function Content({ id }: { id: string }) {
  renders.set(id, (renders.get(id) ?? 0) + 1);
  return <button type="button">{id}</button>;
}

let listRenders = 0;
function List({ pinned = [], layout = "list" }: { pinned?: string[]; layout?: string }) {
  listRenders += 1;
  const list = useVirtualList(50, pinned, layout);
  return (
    <ul>
      {ITEMS.map((id, index) => (
        <VirtualSlot key={id} list={list} id={id} index={index} as="li" label={id} ariaLabel={id}>
          {() => <Content id={id} />}
        </VirtualSlot>
      ))}
    </ul>
  );
}

/** The slots carry their index for the fake observer. */
function tagIndexes() {
  screen.getAllByRole("listitem").forEach((item, index) => {
    item.dataset.index = String(index);
  });
}

function rendered() {
  return screen.queryAllByRole("button").length;
}

function slot(id: string) {
  return screen.getByRole("listitem", { name: id });
}

describe("useVirtualList (#17)", () => {
  beforeEach(() => {
    renders.clear();
    vi.stubGlobal("IntersectionObserver", Fake);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    Fake.last = null;
  });

  it("renders the first items until the observer answers, then only the near ones", () => {
    render(<List />);
    tagIndexes();
    expect(rendered()).toBe(24);
    expect(screen.getAllByRole("listitem")).toHaveLength(100);

    Fake.last!.report((target) => indexOf(target) >= 40 && indexOf(target) < 50);
    expect(rendered()).toBe(10);
    expect(screen.getByRole("button", { name: "item-45" })).toBeInTheDocument();
    expect(slot("item-0")).toHaveStyle({ height: "50px" });
    expect(slot("item-0")).toHaveAttribute("data-virtual", "placeholder");
    expect(slot("item-45")).not.toHaveAttribute("data-virtual");
  });

  it("re-renders only the slots whose state a scroll changed, never the list", () => {
    render(<List />);
    tagIndexes();
    Fake.last!.report((target) => indexOf(target) < 10);
    const before = new Map(renders);
    const listBefore = listRenders;
    // Scrolling by one: item 0 leaves, item 10 arrives.
    Fake.last!.report((target) => indexOf(target) >= 1 && indexOf(target) < 11);
    const again = ITEMS.filter((id) => (renders.get(id) ?? 0) !== (before.get(id) ?? 0));
    expect(again).toEqual(["item-10"]);
    expect(listRenders).toBe(listBefore);
  });

  it("names every placeholder, so screen readers and find-in-page reach every item", () => {
    render(<List />);
    tagIndexes();
    Fake.last!.report(() => false);
    expect(rendered()).toBe(0);
    expect(slot("item-80")).toHaveTextContent("item-80");
  });

  it("keeps the height a slot had when it was last rendered", () => {
    render(<List />);
    tagIndexes();
    Fake.last!.report(() => true);
    expect(rendered()).toBe(100);
    Fake.last!.report((target) => indexOf(target) !== 3, 87);
    expect(slot("item-3")).toHaveStyle({ height: "87px" });
    expect(rendered()).toBe(99);
  });

  it("forgets heights on a layout change and for an item that stops being pinned", () => {
    const view = render(<List pinned={["item-5"]} />);
    tagIndexes();
    Fake.last!.report(() => true);
    // Off screen: item 5 stays rendered (pinned), the others remember 120 px.
    Fake.last!.report(() => false, 120);
    expect(screen.getByRole("button", { name: "item-5" })).toBeInTheDocument();
    expect(slot("item-6")).toHaveStyle({ height: "120px" });

    // Unpinned: its height measured while open does not carry over.
    view.rerender(<List pinned={[]} />);
    Fake.last!.report(() => false, 120);
    expect(slot("item-5")).toHaveStyle({ height: "50px" });

    view.rerender(<List pinned={[]} layout="grid" />);
    tagIndexes();
    Fake.last!.report(() => false);
    expect(slot("item-6")).toHaveStyle({ height: "50px" });
  });

  it("always renders pinned items and a slot that holds the focus", () => {
    render(<List pinned={["item-90"]} />);
    tagIndexes();
    Fake.last!.report((target) => indexOf(target) < 10);
    act(() => screen.getByRole("button", { name: "item-2" }).focus());
    Fake.last!.report(() => false);
    expect(screen.getByRole("button", { name: "item-90" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "item-2" })).toHaveFocus();
    expect(rendered()).toBe(2);
  });

  it("renders everything without IntersectionObserver", () => {
    vi.stubGlobal("IntersectionObserver", undefined);
    render(<List />);
    expect(rendered()).toBe(100);
  });
});
