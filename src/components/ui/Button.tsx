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
  // A caller's own height, padding, radius or font size replaces the default: two utilities for
  // one property would leave the winner to CSS order, not to the caller.
  const own = (re: RegExp, cls: string) => (re.test(className ?? "") ? null : cls);
  return (
    <button
      type="button"
      className={clsx(
        "inline-flex items-center justify-center gap-1.5 font-semibold whitespace-nowrap",
        own(/(^|\s)text-\d/, "text-13"),
        own(/(^|\s)h-/, sm ? "h-7" : pill ? "h-10" : "h-[var(--control-h)]"),
        own(/(^|\s)px-/, sm ? "px-3" : pill ? "px-5" : "px-3.5"),
        own(/(^|\s)rounded-/, pill ? "rounded-full" : "rounded-control"),
        "transition-colors duration-[var(--dur-quick)] focus-visible:shadow-[var(--focus-ring)] disabled:pointer-events-none",
        styles[variant],
        className,
      )}
      {...rest}
    />
  );
}
