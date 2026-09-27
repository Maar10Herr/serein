import type { ComponentChildren } from "preact";

const shapes: Record<string, ComponentChildren> = {
  mark: (
    <>
      <path d="M4.2 5.5c2.3-2 5.2-3 8.2-2.7 4.5.4 7.6 3.8 8.1 7.8.3 2.7-1.2 4.3-3.7 4.3" />
      <path d="M3.1 10.5c.9-2.4 3.1-3.8 5.7-3.4 2.7.4 4.2 2.5 4.2 4.8" />
      <path d="M5.3 18.4c.4-3.1 2.5-5.3 5.1-5.6 1.8-.2 3.1.8 3.4 2.5" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3.2" />
      <path d="m19.1 13.6 1.2 1-.9 1.7-1.6-.4a6.9 6.9 0 0 1-1.3 1.3l.3 1.7-1.8.8-1-1.3a7 7 0 0 1-1.9 0l-1 1.3-1.8-.8.3-1.7a6.9 6.9 0 0 1-1.3-1.3l-1.6.4-.9-1.7 1.2-1a7.2 7.2 0 0 1 0-1.9l-1.2-1 .9-1.7 1.6.4A6.9 6.9 0 0 1 9 8.1l-.3-1.7 1.8-.8 1 1.3a7 7 0 0 1 1.9 0l1-1.3 1.8.8-.3 1.7a6.9 6.9 0 0 1 1.3 1.3l1.6-.4.9 1.7-1.2 1a7.2 7.2 0 0 1 0 1.9Z" />
    </>
  ),
  exclude: (
    <>
      <path d="M12 3 20 6v5c0 4.7-3.2 8-8 10-4.8-2-8-5.3-8-10V6l8-3Z" />
      <path d="M9 12h6" />
    </>
  ),
  include: (
    <>
      <path d="M12 3 20 6v5c0 4.7-3.2 8-8 10-4.8-2-8-5.3-8-10V6l8-3Z" />
      <path d="m8.7 12.2 2.1 2.1 4.7-4.8" />
    </>
  ),
  private: (
    <>
      <path d="M3 3 21 21" />
      <path d="M10.6 10.7a2 2 0 0 0 2.7 2.7" />
      <path d="M9.9 5.2A11.3 11.3 0 0 1 12 5c5.4 0 9 4.7 9.8 6-.4.7-1.6 2.3-3.3 3.6" />
      <path d="M6.2 6.3C4.1 7.6 2.7 9.7 2.2 11c.8 1.3 4.4 6 9.8 6 1.2 0 2.3-.2 3.3-.7" />
    </>
  ),
  resume: (
    <>
      <path d="M2.2 12s3.6-6 9.8-6 9.8 6 9.8 6-3.6 6-9.8 6-9.8-6-9.8-6Z" />
      <circle cx="12" cy="12" r="2.6" />
    </>
  ),
  device: (
    <>
      <rect x="5" y="4" width="14" height="13" rx="1.8" />
      <path d="M2.8 20h18.4l-1.3-3H4.1l-1.3 3Z" />
      <path d="M10 17h4" />
    </>
  ),
  link: (
    <>
      <path d="m9.6 14.4 4.8-4.8" />
      <path d="M7.3 15.8 5.9 17.2a3.3 3.3 0 0 1-4.7-4.7l4.1-4.1A3.3 3.3 0 0 1 10 8.3" />
      <path d="m16.7 8.2 1.4-1.4a3.3 3.3 0 0 1 4.7 4.7l-4.1 4.1A3.3 3.3 0 0 1 14 15.7" />
    </>
  ),
  check: <path d="m5 12.5 4.5 4.2L19 7.5" />,
  copy: (
    <>
      <rect x="8" y="8" width="12" height="13" rx="2" />
      <path d="M16 8V5.8A1.8 1.8 0 0 0 14.2 4H5.8A1.8 1.8 0 0 0 4 5.8v9.4A1.8 1.8 0 0 0 5.8 17H8" />
    </>
  ),
  forget: (
    <>
      <path d="M4 7h16" />
      <path d="m6 7 .8 12.2a1.9 1.9 0 0 0 1.9 1.8h6.6a1.9 1.9 0 0 0 1.9-1.8L18 7" />
      <path d="M9 7V4.8A1.8 1.8 0 0 1 10.8 3h2.4A1.8 1.8 0 0 1 15 4.8V7" />
      <path d="M10 11v6M14 11v6" />
    </>
  ),
  why: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M9.7 9.4a2.4 2.4 0 1 1 4.1 1.7c-1.1 1-1.8 1.4-1.8 2.7" />
      <path d="M12 17.1h.01" />
    </>
  ),
  context: (
    <>
      <path d="M6 3h8l4 4v14H6zM14 3v5h4M9 12h6M9 16h4" />
    </>
  ),
  storage: (
    <>
      <path d="M4 6c0-4 16-4 16 0s-16 4-16 0v12c0 4 16 4 16 0V6M4 12c0 4 16 4 16 0" />
    </>
  ),
  arrow: <path d="M5 12h14M14 7l5 5-5 5" />,
  search: (
    <>
      <circle cx="10" cy="10" r="8" />
      <path d="m16 16 6 6" />
    </>
  ),
  sun: (
    <>
      <path d="M12 3v2M12 19v2M3 12h2M19 12h2M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2" />
      <circle cx="12" cy="12" r="4" />
    </>
  ),
};

export function Icon({ name, size = 20 }: { name: string; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {shapes[name] || shapes.context}
    </svg>
  );
}
