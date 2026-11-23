// The one modal dialog and drawer primitive (#11, M29). Every dialog renders
// its surface through `Modal`, which owns the overlay, focus and keyboard:
//
// - On open, focus moves into the dialog: an element that already took focus
//   (`autoFocus`), else the first element marked `data-autofocus`, else the
//   first form field (`initialFocus="field"`), else the dialog itself.
// - Tab and Shift+Tab wrap inside the dialog.
// - Everything outside the dialog is `inert` while it is open (each sibling
//   on the path from the dialog up to `<body>`), except subtrees marked
//   `data-modal-exempt` (the notification stack, so toasts stay announced).
// - Escape closes the top dialog only; the backdrop closes it too.
// - On close, focus returns to the element that had it when the dialog
//   opened, or, when that element is gone (now or once a refetch removes
//   it), to the dialog below, or to the page heading (`data-focus-fallback`).
//
// Open dialogs form a stack, so a confirm or review opened over a dialog
// takes the keyboard and the inert shell from it, is drawn above it, and
// gives both back.

import { useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";

export type ModalProps = {
  /** Escape, the backdrop, and every Close/Cancel button the dialog renders. */
  onClose: () => void;
  /** False while the dialog must stay open (a write in flight): Escape and the backdrop do nothing. */
  dismissible?: boolean;
  role?: "dialog" | "alertdialog";
  /** Id of the visible title; or `label` when the dialog has none. */
  labelledBy?: string;
  label?: string;
  /** Id of the text that says what the dialog asks (a confirmation's question). */
  describedBy?: string;
  /** Class of the dialog surface. */
  className: string;
  /** Class of the overlay. */
  backdropClassName?: string;
  /** The surface element; the markup each dialog had before M29. */
  element?: "div" | "section" | "aside";
  /**
   * Where focus starts when no element took it or is marked `data-autofocus`:
   * the first form field, else the dialog (`field`); or the dialog itself
   * (`dialog`, for read-and-review surfaces).
   */
  initialFocus?: "field" | "dialog";
  /** Renders into `<body>` (the card detail modals, whose cards clip fixed children). */
  portal?: boolean;
  children: ReactNode;
};

/** Marks a subtree that stays live while a dialog is open. */
const MODAL_EXEMPT = "data-modal-exempt";
/** Marks the element focus returns to when a dialog's opener is gone. */
const FOCUS_FALLBACK = "data-focus-fallback";
const INERT_MARK = "data-modal-inert";

type ModalEntry = {
  layer: HTMLElement;
  dialog: HTMLElement;
  trigger: HTMLElement | null;
  close: () => void;
  dismissible: () => boolean;
};

const stack: ModalEntry[] = [];

const CANDIDATES = [
  "a[href]",
  "button",
  "input",
  "select",
  "textarea",
  "summary",
  "iframe",
  "[tabindex]",
  "[contenteditable]:not([contenteditable='false'])",
].join(", ");
const FIELDS = "input, select, textarea, [contenteditable]:not([contenteditable='false'])";

function ownedBy(element: Element, dialog: HTMLElement): boolean {
  return element.closest("[aria-modal='true']") === dialog;
}

function isFocusable(element: HTMLElement, dialog: HTMLElement): boolean {
  if (element.matches(":disabled") || (element instanceof HTMLInputElement && element.type === "hidden")) {
    return false;
  }
  if (element.closest("[inert], [hidden]") || !ownedBy(element, dialog)) {
    return false;
  }
  // Collapsed or `display: none` content; engines without it (jsdom) skip the check.
  return typeof element.checkVisibility !== "function" || element.checkVisibility();
}

/** The dialog's Tab order (positive `tabindex` is not used in this app). */
export function tabbableElements(dialog: HTMLElement): HTMLElement[] {
  const candidates = Array.from(dialog.querySelectorAll<HTMLElement>(CANDIDATES)).filter(
    (element) => element.tabIndex >= 0 && isFocusable(element, dialog),
  );
  // A radio group is one Tab stop: its checked radio, else its first.
  const checkedRadios = new Map<string, HTMLInputElement>();
  for (const element of candidates) {
    if (element instanceof HTMLInputElement && element.type === "radio" && element.name && element.checked) {
      checkedRadios.set(element.name, element);
    }
  }
  const seenGroups = new Set<string>();
  return candidates.filter((element) => {
    if (!(element instanceof HTMLInputElement) || element.type !== "radio" || !element.name) {
      return true;
    }
    const checked = checkedRadios.get(element.name);
    if (checked) {
      return checked === element;
    }
    if (seenGroups.has(element.name)) {
      return false;
    }
    seenGroups.add(element.name);
    return true;
  });
}

function focusInitial(entry: ModalEntry, initialFocus: "field" | "dialog") {
  const { dialog } = entry;
  const active = document.activeElement;
  if (active instanceof HTMLElement && active !== dialog && dialog.contains(active) && ownedBy(active, dialog)) {
    return;
  }
  const marked = Array.from(dialog.querySelectorAll<HTMLElement>("[data-autofocus]")).find((element) =>
    isFocusable(element, dialog),
  );
  const field =
    initialFocus === "field" ? tabbableElements(dialog).find((element) => element.matches(FIELDS)) : undefined;
  (marked ?? field ?? dialog).focus();
}

function hideTree(element: Element) {
  if (element.hasAttribute("inert") || element.hasAttribute(MODAL_EXEMPT) || element.matches("script, style, link")) {
    return;
  }
  if (element.querySelector(`[${MODAL_EXEMPT}]`)) {
    for (const child of Array.from(element.children)) {
      hideTree(child);
    }
    return;
  }
  element.setAttribute("inert", "");
  element.setAttribute(INERT_MARK, "");
}

/** Makes the shell inert around the top dialog's layer; clears it when none is open. */
function syncInert() {
  for (const element of Array.from(document.querySelectorAll(`[${INERT_MARK}]`))) {
    element.removeAttribute("inert");
    element.removeAttribute(INERT_MARK);
  }
  const top = stack[stack.length - 1];
  if (!top) {
    return;
  }
  let node: Element = top.layer;
  while (node !== document.body && node.parentElement) {
    const parent: HTMLElement = node.parentElement;
    for (const sibling of Array.from(parent.children)) {
      if (sibling !== node) {
        hideTree(sibling);
      }
    }
    node = parent;
  }
}

function trapTab(event: KeyboardEvent, dialog: HTMLElement) {
  const items = tabbableElements(dialog);
  const active = document.activeElement;
  const backward = event.shiftKey;
  let target: HTMLElement | undefined;
  if (items.length === 0) {
    target = dialog;
  } else if (!(active instanceof HTMLElement) || !dialog.contains(active) || active === dialog) {
    target = backward ? items[items.length - 1] : items[0];
  } else {
    const index = items.indexOf(active);
    if (index === -1) {
      // Focus sits on an element outside the Tab order: go to its neighbor in document order.
      const follows = (element: HTMLElement) =>
        Boolean(active.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING);
      target = backward
        ? ([...items].reverse().find((element) => !follows(element)) ?? items[items.length - 1])
        : (items.find(follows) ?? items[0]);
    } else if (backward && index === 0) {
      target = items[items.length - 1];
    } else if (!backward && index === items.length - 1) {
      target = items[0];
    }
  }
  if (target) {
    event.preventDefault();
    target.focus();
  }
}

function handleKeyDown(event: KeyboardEvent) {
  const top = stack[stack.length - 1];
  // A control inside the dialog that used the key (a select closing its
  // menu, an editor indenting) keeps it.
  if (!top || event.defaultPrevented || event.isComposing) {
    return;
  }
  if (event.key === "Escape") {
    event.preventDefault();
    if (top.dismissible()) {
      top.close();
    }
  } else if (event.key === "Tab" && !event.altKey && !event.ctrlKey && !event.metaKey) {
    trapTab(event, top.dialog);
  }
}

function canTakeFocus(element: HTMLElement | null): element is HTMLElement {
  if (!element || !element.isConnected || element.closest("[inert]")) {
    return false;
  }
  const top = stack[stack.length - 1];
  return !top || top.dialog.contains(element);
}

/** Where focus goes when a dialog's opener is gone: the dialog below, else the page heading. */
function fallbackTarget(): HTMLElement | null {
  const top = stack[stack.length - 1];
  return top ? top.dialog : document.querySelector<HTMLElement>(`[${FOCUS_FALLBACK}]`);
}

/**
 * Dialogs closed in the current commit. A dialog whose opener sits inside one
 * of them (a confirm opened from a dialog, both closing together) returns
 * focus to that dialog's opener instead.
 */
let closing: ModalEntry[] = [];

/**
 * The element focus was given back to, watched until focus moves on. When it
 * is removed (in the same commit as the dialog, or later when a refetch drops
 * a deleted item's row), focus would fall to `<body>`: it goes to the
 * fallback instead.
 */
let watched: { target: HTMLElement; observer: MutationObserver } | null = null;

function stopWatching() {
  if (watched) {
    watched.observer.disconnect();
    document.removeEventListener("focusin", handleFocusMove);
    watched = null;
  }
}

function handleFocusMove(event: FocusEvent) {
  if (event.target !== watched?.target) {
    stopWatching();
  }
}

function watchRestored(target: HTMLElement) {
  stopWatching();
  const observer = new MutationObserver(() => {
    if (target.isConnected) {
      return;
    }
    stopWatching();
    const active = document.activeElement;
    const fallback = fallbackTarget();
    if ((!active || active === document.body) && canTakeFocus(fallback)) {
      fallback.focus();
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });
  document.addEventListener("focusin", handleFocusMove);
  watched = { target, observer };
}

function restoreFocus(entry: ModalEntry) {
  const active = document.activeElement;
  // Focus that already moved on (the dialog's own action focused something) stays.
  if (active instanceof HTMLElement && active !== document.body && !entry.layer.contains(active)) {
    return;
  }
  let trigger = entry.trigger;
  for (let owner = closing.find((closed) => trigger && closed.layer.contains(trigger)); owner; ) {
    trigger = owner.trigger;
    owner = closing.find((closed) => closed !== owner && trigger && closed.layer.contains(trigger));
  }
  for (const target of [trigger, fallbackTarget()]) {
    if (canTakeFocus(target)) {
      target.focus();
      if (document.activeElement === target) {
        watchRestored(target);
        return;
      }
    }
  }
  if (active instanceof HTMLElement) {
    active.blur();
  }
}

/**
 * Draws the new top dialog above the one below it. Overlays share the page's
 * stacking context (no shell ancestor creates one), but their classes set
 * different z-indexes: a drawer (20) opened from a card detail modal (30)
 * would otherwise sit under it while holding the keyboard.
 */
function raiseAboveBelow(entry: ModalEntry) {
  const below = stack[stack.length - 2];
  if (!below) {
    return;
  }
  const lower = Number.parseInt(getComputedStyle(below.layer).zIndex, 10);
  const own = Number.parseInt(getComputedStyle(entry.layer).zIndex, 10);
  if (Number.isFinite(lower) && !(own > lower)) {
    entry.layer.style.zIndex = String(lower + 1);
  }
}

function openModal(entry: ModalEntry, initialFocus: "field" | "dialog") {
  stopWatching();
  stack.push(entry);
  if (stack.length === 1) {
    window.addEventListener("keydown", handleKeyDown);
  }
  raiseAboveBelow(entry);
  syncInert();
  focusInitial(entry, initialFocus);
}

function closeModal(entry: ModalEntry) {
  const index = stack.indexOf(entry);
  if (index === -1) {
    return;
  }
  stack.splice(index, 1);
  if (stack.length === 0) {
    window.removeEventListener("keydown", handleKeyDown);
  }
  syncInert();
  restoreFocus(entry);
  if (closing.length === 0) {
    queueMicrotask(() => {
      closing = [];
    });
  }
  closing.push(entry);
}

export function Modal({
  onClose,
  dismissible = true,
  role = "dialog",
  labelledBy,
  label,
  describedBy,
  className,
  backdropClassName = "dialog-backdrop",
  element = "div",
  initialFocus = "field",
  portal = false,
  children,
}: ModalProps) {
  const layerRef = useRef<HTMLDivElement | null>(null);
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const latest = useRef({ onClose, dismissible });

  // The element focused while the dialog first renders, before a child with
  // `autoFocus` can take focus during the commit.
  const [opener] = useState(() => document.activeElement);

  useLayoutEffect(() => {
    latest.current = { onClose, dismissible };
  });

  useLayoutEffect(() => {
    const dialog = dialogRef.current!;
    // Focus as the dialog opens (a dialog closed in this commit has already
    // given it back), unless a child of this dialog already took it.
    const active = document.activeElement;
    const current = active && !dialog.contains(active) ? active : opener;
    const entry: ModalEntry = {
      layer: layerRef.current!,
      dialog,
      trigger: current instanceof HTMLElement && current !== document.body ? current : null,
      close: () => latest.current.onClose(),
      dismissible: () => latest.current.dismissible,
    };
    openModal(entry, initialFocus);
    return () => closeModal(entry);
    // Opens once: a re-render must not move the dialog up the stack or refocus it.
  }, []);

  const Surface = element as "div";
  const layer = (
    <div
      ref={layerRef}
      className={backdropClassName}
      role="presentation"
      onClick={(event) => {
        if (event.target === event.currentTarget && latest.current.dismissible) {
          latest.current.onClose();
        }
      }}
    >
      <Surface
        ref={dialogRef as RefObject<HTMLDivElement>}
        className={className}
        role={role}
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-label={label}
        aria-describedby={describedBy}
        tabIndex={-1}
        onClick={(event) => event.stopPropagation()}
      >
        {children}
      </Surface>
    </div>
  );
  return portal ? createPortal(layer, document.body) : layer;
}
