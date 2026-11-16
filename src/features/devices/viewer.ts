// Loads the shared remote viewer (public/remote-viewer/, the same files HomeLab ships; see
// scripts/sync-remote-viewer.mjs and viewer.test.ts) and opens a session with it.

export type ViewerHandle = { close: () => void };
type ViewerApi = {
  open: (o: {
    mount: HTMLElement;
    url: string;
    auth?: string;
    title: string;
    /** `why`: the host's bye code (`auth`, `removed`, ...) when it ended the session. */
    onClose: (why?: string) => void;
  }) => ViewerHandle;
};

declare global {
  interface Window {
    RemoteViewer?: ViewerApi;
  }
}

const BASE = "/remote-viewer/remote-viewer";
let loading: Promise<ViewerApi> | null = null;

/** Adds the viewer's CSS and script once; rejects if the files are not in this build. */
export function loadViewer(): Promise<ViewerApi> {
  if (window.RemoteViewer) return Promise.resolve(window.RemoteViewer);
  loading ??= new Promise<ViewerApi>((resolve, reject) => {
    const css = document.createElement("link");
    css.rel = "stylesheet";
    css.href = `${BASE}.css`;
    document.head.append(css);
    const js = document.createElement("script");
    js.src = `${BASE}.js`;
    js.onload = () =>
      window.RemoteViewer ? resolve(window.RemoteViewer) : reject(new Error("no RemoteViewer"));
    js.onerror = () => reject(new Error("viewer missing"));
    document.head.append(js);
  }).catch((e: unknown) => {
    loading = null; // a later try may succeed
    throw e;
  });
  return loading;
}
