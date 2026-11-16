import * as Tooltip from "@radix-ui/react-tooltip";
import { QueryClientProvider } from "@tanstack/react-query";
import { Component, type ErrorInfo, type ReactNode } from "react";
import { queryClient } from "@/lib/queryClient";
import { logError } from "@/lib/report";
import { t } from "@/lib/i18n";
import { TitleBar } from "./TitleBar";

export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    logError(`${error.message}\n${info.componentStack ?? ""}`);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    // frameless transparent window: keep the opaque base, drag region and window controls
    return (
      <QueryClientProvider client={queryClient}>
        <Tooltip.Provider>
          <div className="app-win flex flex-col">
            <div className="grain" aria-hidden />
            <TitleBar minimal />
            <div
              role="alert"
              className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8"
            >
              <h1 className="text-16 font-semibold text-text-1">{t("error.title")}</h1>
              <p className="max-w-[480px] text-center text-13 text-text-2">{error.message}</p>
              <button
                type="button"
                className="rounded-control bg-surface-3 px-3 py-1.5 text-13 text-text-1 hover:bg-surface-4"
                onClick={() => window.location.reload()}
              >
                {t("error.reload")}
              </button>
            </div>
          </div>
        </Tooltip.Provider>
      </QueryClientProvider>
    );
  }
}
