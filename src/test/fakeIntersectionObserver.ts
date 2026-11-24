// A stand-in IntersectionObserver whose answers a test gives (#17): install it
// with `vi.stubGlobal("IntersectionObserver", FakeIntersectionObserver)`, then
// `FakeIntersectionObserver.last.report(...)` delivers one batch.

import { act } from "@testing-library/react";

export class FakeIntersectionObserver {
  static last: FakeIntersectionObserver | null = null;
  readonly targets = new Set<Element>();

  constructor(private readonly callback: IntersectionObserverCallback) {
    FakeIntersectionObserver.last = this;
  }

  observe(target: Element) {
    this.targets.add(target);
  }

  unobserve(target: Element) {
    this.targets.delete(target);
  }

  disconnect() {
    this.targets.clear();
  }

  /** Reports every observed target: `near(target)` says whether it intersects; `height` is its measured height. */
  report(near: (target: HTMLElement) => boolean, height = 0) {
    const entries = [...this.targets].map(
      (target) =>
        ({
          target,
          isIntersecting: near(target as HTMLElement),
          boundingClientRect: { height } as DOMRectReadOnly,
        }) as IntersectionObserverEntry,
    );
    act(() => this.callback(entries, this as unknown as IntersectionObserver));
  }
}
