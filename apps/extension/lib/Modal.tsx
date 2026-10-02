import type { ComponentChildren } from "preact";
import { useLayoutEffect, useRef } from "preact/hooks";

const tabbableSelector = [
  "a[href]",
  "area[href]",
  "button",
  "input",
  "select",
  "textarea",
  "iframe",
  "object",
  "embed",
  "summary",
  "audio[controls]",
  "video[controls]",
  '[contenteditable="true"]',
  "[tabindex]",
].join(",");

function isRenderedAndUsable(
  element: HTMLElement,
  boundary?: HTMLElement,
): boolean {
  if (!element.isConnected || element.matches(":disabled")) return false;
  if (boundary && !boundary.contains(element)) return false;

  let current: HTMLElement | null = element;
  while (current) {
    if (
      current.hidden ||
      current.inert ||
      current.getAttribute("aria-hidden")?.toLowerCase() === "true"
    )
      return false;

    const style = window.getComputedStyle(current);
    if (
      style.display === "none" ||
      style.visibility === "hidden" ||
      style.visibility === "collapse" ||
      style.getPropertyValue("content-visibility") === "hidden"
    )
      return false;

    if (current === boundary) return element.getClientRects().length > 0;
    if (current === document.body) break;
    current = current.parentElement;
  }

  return !boundary && element.getClientRects().length > 0;
}

function getTabbables(dialog: HTMLElement): HTMLElement[] {
  return Array.from(dialog.querySelectorAll<HTMLElement>(tabbableSelector))
    .map((element, domOrder) => ({ element, domOrder }))
    .filter(
      ({ element }) =>
        element.tabIndex >= 0 &&
        !(element instanceof HTMLInputElement && element.type === "hidden") &&
        isRenderedAndUsable(element, dialog),
    )
    .sort((a, b) => {
      const aIndex = a.element.tabIndex;
      const bIndex = b.element.tabIndex;
      if (aIndex > 0 && bIndex > 0)
        return aIndex - bIndex || a.domOrder - b.domOrder;
      if (aIndex > 0) return -1;
      if (bIndex > 0) return 1;
      return a.domOrder - b.domOrder;
    })
    .map(({ element }) => element);
}

function makeBackgroundInert(backdrop: HTMLElement): () => void {
  const previousStates: Array<{ element: HTMLElement; wasInert: boolean }> = [];
  let branch: Element = backdrop;

  for (
    let parent = branch.parentElement;
    parent;
    branch = parent, parent = parent.parentElement
  ) {
    for (const sibling of Array.from(parent.children)) {
      if (sibling === branch || !(sibling instanceof HTMLElement)) continue;
      previousStates.push({ element: sibling, wasInert: sibling.inert });
      sibling.inert = true;
    }
    if (parent === document.body) break;
  }

  return () => {
    for (const { element, wasInert } of previousStates.reverse())
      element.inert = wasInert;
  };
}

function focusDashboardFallback() {
  const fallback = document.querySelector<HTMLElement>("main h1, main");
  if (!fallback || !fallback.isConnected) return;

  // Headings and main are not normally in the tab order. Giving the stable
  // fallback programmatic focus does not add it to ordinary Tab traversal.
  if (!fallback.hasAttribute("tabindex"))
    fallback.setAttribute("tabindex", "-1");
  fallback.focus({ preventScroll: true });
}

export function Modal({
  title,
  onClose,
  returnFocusTo,
  children,
}: {
  title: string;
  onClose: () => void;
  returnFocusTo?: HTMLElement | null;
  children: ComponentChildren;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const backdropRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useLayoutEffect(() => {
    const previous =
      returnFocusTo ??
      (document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null);
    const dialog = ref.current;
    const backdrop = backdropRef.current;
    if (!dialog || !backdrop) return;

    const restoreBackground = makeBackgroundInert(backdrop);
    const tabbables = getTabbables(dialog);
    (tabbables[0] ?? dialog).focus({ preventScroll: true });

    const listener = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCloseRef.current();
      }
      if (e.key === "Tab") {
        const currentTabbables = getTabbables(dialog);
        const first = currentTabbables[0];
        const last = currentTabbables.at(-1);
        const active = document.activeElement;

        if (!first) {
          e.preventDefault();
          dialog.focus({ preventScroll: true });
        } else if (
          !dialog.contains(active) ||
          (e.shiftKey &&
            (active === first ||
              !currentTabbables.includes(active as HTMLElement)))
        ) {
          e.preventDefault();
          (e.shiftKey ? last : first)?.focus({ preventScroll: true });
        } else if (
          !e.shiftKey &&
          (active === last || !currentTabbables.includes(active as HTMLElement))
        ) {
          e.preventDefault();
          first.focus({ preventScroll: true });
        }
      }
    };
    document.addEventListener("keydown", listener, true);
    return () => {
      document.removeEventListener("keydown", listener, true);
      restoreBackground();

      if (
        previous &&
        isRenderedAndUsable(previous) &&
        (previous.tabIndex >= 0 || previous.hasAttribute("tabindex"))
      ) {
        previous.focus({ preventScroll: true });
        if (document.activeElement === previous) return;
      }
      focusDashboardFallback();
    };
  }, []);
  return (
    <div
      class="modal-backdrop"
      ref={backdropRef}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        class="modal"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        ref={ref}
      >
        <div class="row between">
          <h2>{title}</h2>
          <button class="ghost" aria-label="Close" onClick={onClose}>
            ×
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}
