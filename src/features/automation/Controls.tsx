import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { call } from "@/lib/ipc";
import { t } from "@/lib/i18n";
import { reportError } from "@/lib/report";

type Control = {
  kind?: string;
  name?: string;
  value?: unknown;
  min?: number;
  max?: number;
  step?: number;
  decimals?: number;
};

const field =
  "h-[var(--control-h)] w-36 rounded-control bg-field px-2.5 text-13 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline)] placeholder:text-text-3 hover:shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus-visible:shadow-[var(--focus-ring)]";

function Field({ c, fire }: { c: Control; fire: (v: unknown) => void }) {
  const num = c.kind === "number";
  const [draft, setDraft] = useState(String(c.value ?? ""));
  // Enter then blur must not fire twice; an empty or non-numeric number never fires.
  const last = useRef(draft);
  // a server-side change (not our own commit echoing back) resets the draft
  const server = String(c.value ?? "");
  useEffect(() => {
    if (server === last.current) return;
    last.current = server;
    setDraft(server);
  }, [server]);
  const commit = () => {
    let v: string | number = draft;
    if (num) {
      if (draft.trim() === "" || !Number.isFinite(Number(draft))) return;
      v = Number(draft);
      if (c.decimals !== undefined) v = Number(v.toFixed(Math.min(Math.max(c.decimals, 0), 20)));
    }
    if (String(v) === last.current) return;
    last.current = String(v);
    fire(v);
  };
  return (
    <label className="flex flex-col gap-1 text-12 text-text-3">
      {c.name}
      <input
        className={field}
        type={num ? "number" : "text"}
        min={c.min}
        max={c.max}
        step={c.step ?? (c.decimals ? 10 ** -c.decimals : undefined)}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
    </label>
  );
}

/** Controls created by ui.create, in a flowing grid; `break` forces a new row. */
export function Controls({ account, controls }: { account: string; controls: Control[] }) {
  const fire = (name: string, value: unknown) =>
    void call("automation_ui_event", { account, name, value }).catch(reportError);
  if (!controls.length) return <p className="text-13 text-text-3">{t("auto.noControls")}</p>;
  return (
    <div className="flex flex-wrap items-end gap-3">
      {controls.map((c, i) => {
        const name = String(c.name ?? "");
        switch (c.kind) {
          case "break":
            return <div key={i} className="h-0 basis-full" />;
          case "button":
            return (
              <Button key={name} onClick={() => fire(name, true)}>
                {name}
              </Button>
            );
          case "label":
            return (
              <p
                key={name}
                className="flex min-h-[var(--control-h)] items-center gap-1 py-1 text-13 text-text-2"
              >
                <span className="text-text-3">{name}</span>
                {String(c.value ?? "")}
              </p>
            );
          case "text":
          case "number":
            return <Field key={name} c={c} fire={(v) => fire(name, v)} />;
          default:
            return null;
        }
      })}
    </div>
  );
}
