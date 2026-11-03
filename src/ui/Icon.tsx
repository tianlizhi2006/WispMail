import type { JSX } from "solid-js";

type IconName =
  | "mail"
  | "plus"
  | "arrow"
  | "shield"
  | "settings"
  | "close"
  | "check"
  | "server"
  | "trash"
  | "refresh"
  | "compose"
  | "download"
  | "reply"
  | "forward"
  | "paperclip"
  | "inbox"
  | "star"
  | "archive"
  | "folder";
const paths: Record<IconName, () => JSX.Element> = {
  star: () => (
    <path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3 6.5 20.2l1-6.2L3 9.6l6.2-.9L12 3Z" />
  ),
  archive: () => (
    <>
      <rect x="3" y="4" width="18" height="5" rx="1" />
      <path d="M5 9v10a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V9M10 13h4" />
    </>
  ),
  folder: () => (
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z" />
  ),
  refresh: () => (
    <>
      <path d="M20 7v5h-5M4 17v-5h5" />
      <path d="M6 7a7 7 0 0 1 12-2l2 3M4 16l2 3a7 7 0 0 0 12-2" />
    </>
  ),
  compose: () => (
    <>
      <path d="M20 12v8H4V4h8M10 14l2-5 7-7 3 3-7 7-5 2Z" />
    </>
  ),
  download: () => (
    <>
      <path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5" />
    </>
  ),
  reply: () => <path d="m9 5-6 6 6 6M3 11h10c5 0 8 3 8 8" />,
  forward: () => <path d="m15 5 6 6-6 6m6-6H11c-5 0-8 3-8 8" />,
  paperclip: () => (
    <path d="m9 12 6-6a3 3 0 0 1 4 4l-9 9a5 5 0 0 1-7-7L13 2m3 6-8 8a1 1 0 0 1-2-2l8-8" />
  ),
  mail: () => (
    <>
      <rect x="3" y="5" width="18" height="14" rx="3" />
      <path d="m3 7 9 6 9-6" />
    </>
  ),
  plus: () => <path d="M12 5v14M5 12h14" />,
  arrow: () => <path d="M5 12h14m-5-5 5 5-5 5" />,
  shield: () => (
    <>
      <path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z" />
      <path d="m8 12 3 3 5-6" />
    </>
  ),
  settings: () => (
    <>
      <path d="M4 7h16M4 17h16" />
      <circle cx="9" cy="7" r="3" />
      <circle cx="15" cy="17" r="3" />
    </>
  ),
  close: () => <path d="m6 6 12 12M6 18 18 6" />,
  check: () => <path d="m5 12 4 4L19 6" />,
  server: () => (
    <>
      <rect x="3" y="3" width="18" height="7" rx="2" />
      <rect x="3" y="14" width="18" height="7" rx="2" />
      <path d="M7 6.5h.01M7 17.5h.01M12 6.5h5M12 17.5h5" />
    </>
  ),
  trash: () => (
    <>
      <path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7" />
    </>
  ),
  inbox: () => (
    <>
      <path d="m5 4-3 9v7h20v-7l-3-9H5Z" />
      <path d="M2 13h6l2 3h4l2-3h6" />
    </>
  ),
};

export function Icon(props: { name: IconName; class?: string }) {
  return (
    <svg
      class={props.class}
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.7"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      {paths[props.name]()}
    </svg>
  );
}
