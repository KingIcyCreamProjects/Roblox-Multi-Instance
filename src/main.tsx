import { QueryClientProvider } from "@tanstack/react-query";
import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { ErrorBoundary } from "./app/ErrorBoundary";
import { RemoteIndicator } from "./features/devices/RemoteIndicator";
import { call, inTauri } from "./lib/ipc";
import { queryClient } from "./lib/queryClient";
import { logError } from "./lib/report";
import "./styles/index.css";

// The "being controlled" bar (its own small window, see src-tauri/src/remote) renders alone.
const indicator = location.hash === "#remote-indicator";
// an opaque window: index.css gives it a flat base instead of the dev build's fake desktop
if (indicator) document.documentElement.dataset.indicator = "";

// Native window: acrylic is behind the page. Browser dev build: CSS fakes it (see .app-win).
if (inTauri && !indicator) {
  document.documentElement.dataset.native = "";
  // Only Windows 11 rounds our frameless window; Linux corners stay square, macOS clips its own.
  if (navigator.userAgent.includes("Windows")) document.documentElement.dataset.os = "windows";
  // Transparency effects off: keep the opaque fallback instead of a see-through window.
  void call("app_info")
    .then((i) => i.acrylic && (document.documentElement.dataset.acrylic = ""))
    .catch(() => {});
}

// Last resolved theme, applied before first render so light mode does not flash dark.
try {
  const theme = localStorage.getItem("rmi.theme");
  if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
} catch {
  /* storage unavailable */
}

// ErrorBoundary only sees render errors: handlers, timers and stray promises go to the log file too
const why = (x: unknown) => (x instanceof Error ? (x.stack ?? x.message) : String(x));
addEventListener("error", (e) => logError(`uncaught: ${why(e.error ?? e.message)}`));
addEventListener("unhandledrejection", (e) => logError(`unhandled rejection: ${why(e.reason)}`));

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        {indicator ? <RemoteIndicator /> : <App />}
      </QueryClientProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);
