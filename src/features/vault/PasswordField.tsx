import clsx from "clsx";
import { Eye, EyeOff, Lock } from "lucide-react";
import { motion } from "motion/react";
import { useId, useState, type ComponentProps } from "react";
import { IconButton } from "@/components/ui/IconButton";
import { t } from "@/lib/i18n";

type Props = Omit<ComponentProps<"input">, "type" | "id"> & {
  label: string;
  error?: string | null;
  hint?: string;
  /** visible label; default is a placeholder plus a screen-reader label */
  showLabel?: boolean;
};

export function PasswordField({ label, error, hint, showLabel, className, ...rest }: Props) {
  const id = useId();
  const [shown, setShown] = useState(false);
  const note = error ?? hint;
  return (
    <div className="flex flex-col gap-1.5">
      <label
        htmlFor={id}
        className={clsx("text-13 font-medium text-text-1", !showLabel && "sr-only")}
      >
        {label}
      </label>
      {/* a short shake acknowledges a rejected password; MotionConfig drops it under reduced motion */}
      <motion.div
        className="relative"
        animate={error ? { x: [0, -5, 5, -3, 3, 0] } : { x: 0 }}
        transition={{ duration: 0.28, ease: "easeOut" }}
      >
        <Lock
          size={15}
          strokeWidth={1.75}
          aria-hidden
          className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-text-2"
        />
        <input
          id={id}
          placeholder={label}
          type={shown ? "text" : "password"}
          aria-invalid={!!error}
          aria-describedby={note ? `${id}-note` : undefined}
          className={clsx(
            "h-9 w-full rounded-control bg-field pr-10 pl-9 text-13 text-text-1 placeholder:text-text-3",
            "transition-colors duration-[var(--dur-quick)] focus:shadow-[var(--focus-ring)] disabled:opacity-40",
            error
              ? "shadow-[inset_0_0_0_1px_var(--status-error)]"
              : "shadow-[inset_0_0_0_1px_var(--hairline-strong)]",
            className,
          )}
          {...rest}
        />
        <IconButton
          label={shown ? t("vault.hide") : t("vault.show")}
          aria-pressed={shown}
          onClick={() => setShown((s) => !s)}
          className="absolute top-0.5 right-0.5 size-8"
        >
          {shown ? <EyeOff size={16} strokeWidth={1.75} /> : <Eye size={16} strokeWidth={1.75} />}
        </IconButton>
      </motion.div>
      {note && (
        <p
          id={`${id}-note`}
          role={error ? "alert" : undefined}
          className={clsx("text-13", error ? "text-error" : "text-text-2")}
        >
          {note}
        </p>
      )}
    </div>
  );
}
