import * as Toggle from "@radix-ui/react-toggle-group";
import type { LaunchMode } from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";

const SERVER_MODES: { v: LaunchMode; label: MsgKey; tip: MsgKey }[] = [
  { v: "any", label: "mode.any", tip: "mode.any.d" },
  { v: "smallest", label: "mode.smallest", tip: "mode.smallest.d" },
  { v: "random", label: "mode.random", tip: "mode.random.d" },
  { v: "specific", label: "mode.specific", tip: "dest.jobHint" },
];

/** Server mode segmented control, plus the JobId field when Specific (Inspector and Play once). */
export function ServerPick({
  mode,
  job,
  onMode,
  onJob,
  onEnter,
}: {
  mode: LaunchMode;
  job: string;
  onMode: (m: LaunchMode) => void;
  onJob: (j: string) => void;
  onEnter: () => void;
}) {
  return (
    <>
      <Toggle.Root
        type="single"
        value={mode}
        onValueChange={(v) => v && onMode(v as LaunchMode)}
        aria-label={t("mode.label")}
        className="flex h-8 items-center self-start rounded-control bg-surface-3 p-0.5"
      >
        {SERVER_MODES.map((m) => (
          <Toggle.Item
            key={m.v}
            value={m.v}
            title={t(m.tip)}
            className="h-7 rounded-[calc(var(--radius-control)-2px)] px-2.5 text-12 font-medium text-text-2 transition-colors duration-[var(--dur-quick)] hover:text-text-1 data-[state=on]:bg-surface-4 data-[state=on]:text-text-1"
          >
            {t(m.label)}
          </Toggle.Item>
        ))}
      </Toggle.Root>
      {mode === "specific" && (
        <input
          aria-label={t("server.label")}
          placeholder={t("server.placeholder")}
          value={job}
          spellCheck={false}
          onChange={(e) => onJob(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && onEnter()}
          className="h-8 rounded-control bg-field px-3 font-mono text-12 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline-strong)] placeholder:font-sans placeholder:text-text-3 focus:shadow-[var(--focus-ring)] focus:outline-none"
        />
      )}
    </>
  );
}
