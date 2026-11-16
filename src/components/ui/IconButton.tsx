import * as Tooltip from "@radix-ui/react-tooltip";
import clsx from "clsx";
import type { ButtonHTMLAttributes, ReactNode } from "react";

type Props = ButtonHTMLAttributes<HTMLButtonElement> & { label: string; children: ReactNode };

/** Square icon button with a tooltip that doubles as its accessible name. */
export function IconButton({ label, children, className, ...rest }: Props) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>
        <button
          type="button"
          aria-label={label}
          className={clsx(
            "inline-grid place-items-center rounded-control text-text-2 transition-colors duration-[var(--dur-quick)]",
            "hover:bg-surface-3 hover:text-text-1 active:bg-surface-2 disabled:pointer-events-none disabled:opacity-40",
            className?.includes("size-") ? "" : "size-8",
            className,
          )}
          {...rest}
        >
          {children}
        </button>
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content
          sideOffset={6}
          className="glass z-50 rounded-chip px-2 py-1 text-12 text-text-1"
        >
          {label}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}
