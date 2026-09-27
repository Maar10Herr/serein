import type { ComponentChildren } from "preact";
import { useEffect, useRef } from "preact/hooks";
export function Modal({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ComponentChildren;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement;
    const el = ref.current!;
    const focusables = () =>
      Array.from(
        el.querySelectorAll<HTMLElement>(
          'button,input,textarea,select,[tabindex="0"]',
        ),
      );
    focusables()[0]?.focus();
    const listener = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "Tab") {
        const fs = focusables();
        if (e.shiftKey && document.activeElement === fs[0]) {
          e.preventDefault();
          fs.at(-1)?.focus();
        } else if (!e.shiftKey && document.activeElement === fs.at(-1)) {
          e.preventDefault();
          fs[0]?.focus();
        }
      }
    };
    document.addEventListener("keydown", listener);
    return () => {
      document.removeEventListener("keydown", listener);
      previous?.focus();
    };
  }, []);
  return (
    <div
      class="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        class="modal"
        role="dialog"
        aria-modal="true"
        aria-label={title}
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
