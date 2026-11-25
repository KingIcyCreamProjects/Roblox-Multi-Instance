import * as Switch from "@radix-ui/react-switch";
import clsx from "clsx";
import { TriangleAlert } from "lucide-react";
import { useEffect, useId, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { t, type MsgKey } from "@/lib/i18n";
import type { Item } from "./catalog";

export const inputCls =
  "h-[var(--control-h)] rounded-control border border-hairline-strong bg-surface-2 px-3 text-14 text-text-1 placeholder:text-text-3 " +
  "transition-colors duration-[var(--dur-quick)] hover:bg-surface-3 disabled:pointer-events-none disabled:opacity-40";

/** Sub-heading inside a section; one margin/size everywhere. */
export function SubHeading({ children }: { children: ReactNode }) {
  return <h3 className="mt-5 mb-1 text-14 font-semibold text-text-1">{children}</h3>;
}

/** One padding for every list row inside a section. */
export const listRowCls = "rounded-control px-3 py-2.5 hover:bg-surface-3/60";

/** Label + one-line description on the left, control on the right. */
export function Row({
  id,
  label,
  desc,
  note,
  children,
}: {
  id: string;
  label: string;
  desc: string;
  /** why the control is disabled, shown under the description */
  note?: string | null;
  children: ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-6 border-b border-hairline py-3.5">
      <div className="min-w-0">
        <p id={`${id}-l`} className="text-14 text-text-1">
          {label}
        </p>
        <p id={`${id}-d`} className="text-13 text-text-2">
          {desc}
        </p>
        {/* controls also point aria-describedby at `-n`, so the disabled reason is read out */}
        {note && (
          <p id={`${id}-n`} className="text-13 text-text-3">
            {note}
          </p>
        )}
      </div>
      <div className="flex max-w-[55%] min-w-0 shrink-0 items-center gap-3 pt-0.5">{children}</div>
    </div>
  );
}

export function Toggle({
  id,
  checked,
  disabled,
  onChange,
  label,
  describedBy,
}: {
  id: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
  /** outside a Row: its own name, and what describes it */
  label?: string;
  describedBy?: string;
}) {
  return (
    <Switch.Root
      checked={checked}
      disabled={disabled}
      onCheckedChange={onChange}
      aria-label={label}
      aria-labelledby={label ? undefined : `${id}-l`}
      aria-describedby={describedBy ?? `${id}-d ${id}-n`}
      className={clsx(
        "h-5 w-9 shrink-0 rounded-full bg-[color-mix(in_oklab,var(--text-1)_35%,transparent)] p-0.5 transition-colors duration-[var(--dur-quick)]",
        "hover:bg-[color-mix(in_oklab,var(--text-1)_45%,transparent)] data-[state=checked]:bg-accent data-[state=checked]:hover:bg-accent-hover",
        "disabled:pointer-events-none disabled:opacity-40",
      )}
    >
      <Switch.Thumb className="block size-4 rounded-full bg-[var(--on-image)] shadow-[0_1px_2px_oklch(0_0_0/0.3)] transition-transform duration-[var(--dur-quick)] data-[state=checked]:translate-x-4 data-[state=checked]:bg-accent-ink" />
    </Switch.Root>
  );
}

function NumberCtl({
  id,
  item,
  value,
  disabled,
  onCommit,
}: {
  id: string;
  item: Item;
  value: number;
  disabled: boolean;
  onCommit: (stored: number) => void;
}) {
  const scale = item.scale ?? 1;
  const step = item.step ?? 1;
  const min = item.min! / scale;
  const max = item.max! / scale;
  const shown = value / scale;
  const [txt, setTxt] = useState(String(shown));
  // the value this field just drafted while typing: its echo must not rewrite the text ("1.50")
  const typed = useRef<number | null>(null);
  useEffect(() => {
    if (value !== typed.current) setTxt(String(shown));
    typed.current = null;
  }, [value, shown]);

  // edits go to the draft (no IPC), so every change records at once; Save/Discard own the rest
  const commit = (raw: number) => {
    if (Number.isNaN(raw)) return setTxt(String(shown));
    const c = Math.min(max, Math.max(min, Math.round(raw / step) * step));
    setTxt(String(c));
    const stored = Math.round(c * scale);
    if (stored !== value) onCommit(stored);
  };
  const common = {
    "aria-labelledby": `${id}-l`,
    "aria-describedby": `${id}-d ${id}-n`,
    disabled,
    min,
    max,
    step,
  };
  // a typed 0 is a value, not empty; the range only renders when max - min > 2 (no divide by 0)
  const cur = txt.trim() !== "" && Number.isFinite(Number(txt)) ? Number(txt) : shown;
  const fill = Math.min(100, Math.max(0, ((cur - min) / (max - min)) * 100));
  return (
    <>
      {item.kind === "num" && max - min > 2 && (
        <input
          type="range"
          {...common}
          value={cur}
          onChange={(e) => commit(Number(e.target.value))}
          className="range w-40 disabled:opacity-40"
          style={
            {
              "--fill": `${fill}%`,
            } as CSSProperties
          }
        />
      )}
      <input
        type="number"
        inputMode="decimal"
        {...common}
        value={txt}
        onChange={(e) => {
          const v = e.target.value;
          setTxt(v);
          // in range: drafted now (Save can follow without a blur); out of range waits for blur's clamp
          const n = Number(v);
          if (!v.trim() || !Number.isFinite(n) || n < min || n > max) return;
          const stored = Math.round(n * scale);
          if (stored === value) return;
          typed.current = stored;
          onCommit(stored);
        }}
        onBlur={() => commit(parseFloat(txt))}
        onKeyDown={(e) => e.key === "Enter" && commit(parseFloat(txt))}
        className={clsx(inputCls, "w-24 text-right tabular-nums")}
      />
    </>
  );
}

function TextCtl({
  id,
  item,
  value,
  disabled,
  onCommit,
}: {
  id: string;
  item: Item;
  value: string | null;
  disabled: boolean;
  onCommit: (v: string | null) => void;
}) {
  const [txt, setTxt] = useState(value ?? "");
  useEffect(() => setTxt(value ?? ""), [value]);
  // every keystroke goes to the draft, so Save right after typing keeps it
  const commit = (v: string) => {
    const next = item.nullable && v.trim() === "" ? null : v;
    if (next !== value) onCommit(next);
  };
  return (
    <input
      type="text"
      aria-labelledby={`${id}-l`}
      aria-describedby={`${id}-d ${id}-n`}
      disabled={disabled}
      value={txt}
      data-pii={item.pii || undefined}
      placeholder={item.placeholder && t(item.placeholder)}
      spellCheck={false}
      maxLength={item.max}
      onChange={(e) => {
        setTxt(e.target.value);
        if (!item.onBlur) commit(e.target.value);
      }}
      onBlur={() => item.onBlur && commit(txt)}
      onKeyDown={(e) => item.onBlur && e.key === "Enter" && e.currentTarget.blur()}
      className={clsx(inputCls, "w-72 max-w-full font-mono text-13 placeholder:font-sans")}
    />
  );
}

/** One catalog entry bound to its value. `reason` set = disabled with that explanation. */
export function Setting({
  item,
  value,
  reason,
  onChange,
}: {
  item: Item;
  value: unknown;
  reason: string | null;
  onChange: (v: unknown) => void;
}) {
  const id = useId();
  const key = (suffix = "") => `set.${item.path}${suffix}` as MsgKey;
  const disabled = !!reason;
  let ctl: ReactNode;
  if (item.kind === "bool")
    ctl = <Toggle id={id} checked={!!value} disabled={disabled} onChange={onChange} />;
  else if (item.kind === "enum")
    ctl = (
      <select
        aria-labelledby={`${id}-l`}
        aria-describedby={`${id}-d ${id}-n`}
        disabled={disabled}
        value={String(value)}
        onChange={(e) => onChange(e.target.value)}
        className={clsx(inputCls, "w-44")}
      >
        {item.options!.map((o) => (
          <option key={o} value={o}>
            {t(`opt.${o}` as MsgKey)}
          </option>
        ))}
      </select>
    );
  else if (item.kind === "text")
    ctl = (
      <TextCtl
        id={id}
        item={item}
        value={(value as string | null) ?? null}
        disabled={disabled}
        onCommit={onChange}
      />
    );
  else
    ctl = (
      <NumberCtl
        id={id}
        item={item}
        value={typeof value === "number" ? value : item.min!}
        disabled={disabled}
        onCommit={onChange}
      />
    );
  return (
    <>
      <Row id={id} label={t(key())} desc={t(key(".d"))} note={reason}>
        {ctl}
      </Row>
      {item.warn && (
        <div
          role="note"
          className="mt-2 flex items-start gap-2.5 rounded-control bg-error/10 px-3 py-2.5 text-13 text-text-1"
        >
          <TriangleAlert
            size={16}
            strokeWidth={1.75}
            className="mt-px shrink-0 text-error"
            aria-hidden
          />
          <p>{t(item.warn)}</p>
        </div>
      )}
    </>
  );
}
