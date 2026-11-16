import type { ReactNode } from "react";
import { Mark } from "@/components/Mark";

/** Centered glass mark tile + title + body, shared by the vault gates. */
export function GateHead({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-3.5 text-center">
      <span className="mb-1.5 grid size-14 place-items-center rounded-2xl bg-surface-3 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline-strong),inset_0_1px_0_var(--sheen),var(--shadow-float)]">
        <Mark size={26} />
      </span>
      <h1 className="text-[1.375rem] leading-tight font-semibold tracking-[-0.01em] text-balance text-text-1">
        {title}
      </h1>
      {children}
    </div>
  );
}
