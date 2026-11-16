import clsx from "clsx";
import type { ButtonHTMLAttributes } from "react";

type Variant = "primary" | "secondary" | "ghost" | "danger";

// Round 4 buttons: 32px (28px `sm`), 8px radius, a hairline top highlight on filled ones.
// Pills are only for floating capsule bars (`pill`).
const styles: Record<Variant, string> = {
  primary:
    "bg-accent text-accent-ink font-semibold shadow-[inset_0_1px_0_rgb(255_255_255/0.3)] hover:bg-accent-hover active:bg-accent-press disabled:bg-surface-3 disabled:text-text-1 disabled:shadow-none disabled:opacity-40",
  secondary:
    "bg-surface-3 text-text-1 shadow-[inset_0_1px_0_rgb(255_255_255/0.14)] hover:bg-surface-4 active:bg-surface-2 disabled:opacity-40",
  ghost: "text-text-2 hover:bg-surface-3 hover:text-text-1 active:bg-surface-2 disabled:opacity-40",
  danger:
    "bg-surface-3 text-error shadow-[inset_0_1px_0_rgb(255_255_255/0.14)] hover:bg-surface-4 disabled:opacity-40",
};

export function Button({
  variant = "secondary",
  sm,
  pill,
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; sm?: boolean; pill?: boolean }) {
  return (
    <button
      type="button"
      className={clsx(
        "inline-flex items-center justify-center gap-1.5 text-13 font-semibold whitespace-nowrap",
        sm ? "h-7 px-3" : pill ? "h-10 px-5" : "h-[var(--control-h)] px-3.5",
        pill ? "rounded-full" : "rounded-control",
        "transition-colors duration-[var(--dur-quick)] focus-visible:shadow-[var(--focus-ring)] disabled:pointer-events-none",
        styles[variant],
        className,
      )}
      {...rest}
    />
  );
}
