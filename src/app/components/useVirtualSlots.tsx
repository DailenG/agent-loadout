// Windowed rendering for long lists (#17): every item keeps a slot element
// (one host element, so list semantics, keys and order stay), and only the
// slots near the scroll viewport render their content. A slot whose content
// is not rendered keeps the height it had when last seen (or an estimate),
// so the scrollbar stays put, and holds the item's name as visually hidden
// text, so screen readers and find-in-page still reach every item.
//
// An IntersectionObserver on the list's scroll container decides which slots
// are near; each slot subscribes to its own state, so a scroll re-renders
// only the slots that cross the margin, never the page. Without an observer
// (old engines, jsdom) every slot renders its content. Pinned items (an
// expanded or busy card, a selected row) and a slot holding the focus always
// render theirs.

import { useCallback, useEffect, useRef, useSyncExternalStore, type ReactNode } from "react";

/** How far outside the scroll viewport a slot still renders, in pixels. */
const NEAR_MARGIN_PX = 800;
/** Items rendered at once, before the observer first reports a slot. */
const FIRST_ITEMS = 24;

function scrollParent(element: HTMLElement): HTMLElement | null {
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    const { overflowY } = getComputedStyle(parent);
    if (overflowY === "auto" || overflowY === "scroll" || overflowY === "overlay") {
      return parent;
    }
  }
  return null;
}

/** One list's slot state; see the module comment. */
export class VirtualList {
  private readonly near = new Set<string>();
  private readonly reported = new Set<string>();
  private readonly heights = new Map<string, number>();
  private readonly listeners = new Map<string, Set<() => void>>();
  private readonly elements = new Map<string, HTMLElement>();
  private readonly keyOf = new WeakMap<Element, string>();
  private readonly refs = new Map<string, (element: HTMLElement | null) => void>();
  private observer: IntersectionObserver | null = null;
  private pinned = new Set<string>();
  private layout: string | null = null;

  constructor(readonly estimate: number) {}

  /** Whether item `id` at `index` renders its content. */
  shows(id: string, index: number): boolean {
    return (
      typeof IntersectionObserver === "undefined"
      || this.pinned.has(id)
      || this.near.has(id)
      || (!this.reported.has(id) && index < FIRST_ITEMS)
    );
  }

  /** The placeholder height of `id`: as last rendered, else `estimate`. */
  height(id: string, estimate = this.estimate): number {
    return this.heights.get(id) ?? estimate;
  }

  subscribe(id: string, listener: () => void): () => void {
    let set = this.listeners.get(id);
    if (!set) {
      set = new Set();
      this.listeners.set(id, set);
    }
    set.add(listener);
    return () => {
      set.delete(listener);
    };
  }

  /** Sets the pinned ids; an id that stops being pinned forgets its height (it was measured open or busy). */
  pin(ids: readonly string[]) {
    const next = new Set(ids.filter((id) => id !== ""));
    for (const id of this.pinned) {
      if (!next.has(id)) {
        this.heights.delete(id);
      }
    }
    this.pinned = next;
  }

  /** A new layout (a view mode): heights and reports of the old one no longer apply. */
  setLayout(layout: string) {
    if (this.layout === null || this.layout === layout) {
      this.layout = layout;
      return;
    }
    this.layout = layout;
    this.heights.clear();
    this.near.clear();
    this.reported.clear();
    // Observe again, so every slot is reported afresh.
    if (this.observer) {
      this.observer.disconnect();
      for (const element of this.elements.values()) {
        this.observer.observe(element);
      }
    }
  }

  /** The ref of slot `id`: stable per id, so a re-render never re-observes. */
  ref(id: string): (element: HTMLElement | null) => void {
    let ref = this.refs.get(id);
    if (!ref) {
      ref = (element) => {
        const current = this.elements.get(id);
        if (current) {
          this.observer?.unobserve(current);
          this.elements.delete(id);
        }
        if (element) {
          this.elements.set(id, element);
          this.keyOf.set(element, id);
          this.observe(element);
        } else {
          this.refs.delete(id);
        }
      };
      this.refs.set(id, ref);
    }
    return ref;
  }

  disconnect() {
    this.observer?.disconnect();
    this.observer = null;
  }

  private observe(element: HTMLElement) {
    if (typeof IntersectionObserver === "undefined") {
      return;
    }
    this.observer ??= new IntersectionObserver((entries) => this.update(entries), {
      root: scrollParent(element),
      rootMargin: `${NEAR_MARGIN_PX}px 0px`,
    });
    this.observer.observe(element);
  }

  private update(entries: IntersectionObserverEntry[]) {
    const changed = new Set<string>();
    for (const entry of entries) {
      const id = this.keyOf.get(entry.target);
      if (id === undefined) {
        continue;
      }
      if (!this.reported.has(id)) {
        this.reported.add(id);
        changed.add(id);
      }
      const keep = entry.isIntersecting || entry.target.contains(document.activeElement);
      const rendered = (entry.target as HTMLElement).dataset.virtual !== "placeholder";
      if (!keep && rendered && entry.boundingClientRect.height > 0) {
        // Still rendered now: remember its height for the placeholder.
        this.heights.set(id, entry.boundingClientRect.height);
      }
      if (keep !== this.near.has(id)) {
        if (keep) {
          this.near.add(id);
        } else {
          this.near.delete(id);
        }
        changed.add(id);
      }
    }
    for (const id of changed) {
      this.listeners.get(id)?.forEach((listener) => listener());
    }
  }
}

/**
 * The slot state of one list. `estimate`: the height in pixels of a slot never
 * rendered (one card or row, gap excluded). `pinned`: ids that always render.
 * `layout`: changes when the items' geometry does (a view mode).
 */
export function useVirtualList(estimate: number, pinned: readonly string[], layout = ""): VirtualList {
  const ref = useRef<VirtualList | null>(null);
  ref.current ??= new VirtualList(estimate);
  const list = ref.current;
  list.pin(pinned);
  list.setLayout(layout);
  useEffect(() => () => list.disconnect(), [list]);
  return list;
}

export type VirtualSlotProps = {
  list: VirtualList;
  id: string;
  index: number;
  /** The item's name: the placeholder's visually hidden heading. */
  label: string;
  /** `li` for list items; the placeholder then names the item with `ariaLabel` and a hidden span. */
  as?: "div" | "li";
  /** The slot's class while its content renders. */
  className?: string;
  /** The slot's accessible name, rendered or not. */
  ariaLabel?: string;
  /** This slot's never-rendered height, when not the list's. */
  estimate?: number;
  children: () => ReactNode;
};

export function VirtualSlot({ list, id, index, label, as: Tag = "div", className, ariaLabel, estimate, children }: VirtualSlotProps) {
  const subscribe = useCallback((listener: () => void) => list.subscribe(id, listener), [list, id]);
  const shown = useSyncExternalStore(subscribe, () => list.shows(id, index));
  if (shown) {
    return (
      <Tag ref={list.ref(id)} className={className} aria-label={ariaLabel}>
        {children()}
      </Tag>
    );
  }
  return (
    <Tag
      ref={list.ref(id)}
      className="virtual-slot"
      aria-label={ariaLabel}
      data-virtual="placeholder"
      style={{ height: list.height(id, estimate) }}
    >
      {Tag === "li" ? <span className="sr-only">{label}</span> : <h3 className="sr-only">{label}</h3>}
    </Tag>
  );
}
