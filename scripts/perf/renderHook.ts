// A minimal React DevTools global hook for the perf measurements: on every
// commit it counts the components that actually rendered (did work in this
// commit, not bailed out) and the commit's render duration. React only
// enables its profiling timers when a DevTools hook exists before react-dom
// loads, so this is a setup file that runs first.

type Fiber = {
  tag: number;
  type: unknown;
  flags: number;
  actualStartTime?: number;
  actualDuration?: number;
  child: Fiber | null;
  sibling: Fiber | null;
};

export type CommitSample = {
  /** Render duration of the commit (ms, React's profiler timer). */
  duration: number;
  /** Rendered components by name. */
  rendered: Map<string, number>;
};

export type RenderLog = {
  commits: CommitSample[];
  reset(): void;
};

const PERFORMED_WORK = 1;
// FunctionComponent, ClassComponent, ForwardRef, MemoComponent, SimpleMemoComponent.
const COMPONENT_TAGS = new Set([0, 1, 11, 14, 15]);

function componentName(fiber: Fiber): string {
  const type = fiber.type as { displayName?: string; name?: string; render?: { name?: string }; type?: { name?: string } } | null;
  return type?.displayName || type?.name || type?.render?.name || type?.type?.name || "Anonymous";
}

const log: RenderLog = {
  commits: [],
  reset() {
    log.commits.length = 0;
  },
};
let lastCommit = 0;

(globalThis as Record<string, unknown>).__REACT_DEVTOOLS_GLOBAL_HOOK__ = {
  isDisabled: false,
  supportsFiber: true,
  renderers: new Map(),
  inject: () => 1,
  checkDCE: () => {},
  onScheduleFiberRoot: () => {},
  onCommitFiberUnmount: () => {},
  onPostCommitFiberRoot: () => {},
  onCommitFiberRoot(_id: number, root: { current: Fiber }) {
    const rendered = new Map<string, number>();
    const stack: Fiber[] = root.current.child ? [root.current.child] : [];
    while (stack.length > 0) {
      const fiber = stack.pop()!;
      if (
        COMPONENT_TAGS.has(fiber.tag) &&
        (fiber.flags & PERFORMED_WORK) !== 0 &&
        (fiber.actualStartTime ?? 0) >= lastCommit
      ) {
        const name = componentName(fiber);
        rendered.set(name, (rendered.get(name) ?? 0) + 1);
      }
      if (fiber.sibling) stack.push(fiber.sibling);
      if (fiber.child) stack.push(fiber.child);
    }
    log.commits.push({ duration: root.current.actualDuration ?? 0, rendered });
    lastCommit = performance.now();
  },
};

(globalThis as Record<string, unknown>).__perfRenderLog = log;
