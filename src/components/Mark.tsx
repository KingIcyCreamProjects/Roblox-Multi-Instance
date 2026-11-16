// RMI mark: three offset rounded squares (stacked instances). Monochrome; colour from currentColor.
export function Mark({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      style={{ zoom: "var(--ui-scale, 1)" }}
      className={className}
    >
      <rect
        x="2"
        y="2"
        width="12"
        height="12"
        rx="3.2"
        stroke="currentColor"
        strokeWidth="2"
        opacity="0.45"
      />
      <rect
        x="6"
        y="6"
        width="12"
        height="12"
        rx="3.2"
        stroke="currentColor"
        strokeWidth="2"
        opacity="0.7"
      />
      <rect x="10" y="10" width="12" height="12" rx="3.2" fill="currentColor" />
    </svg>
  );
}
