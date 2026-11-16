import { LogIn, FolderInput } from "lucide-react";
import { Mark } from "@/components/Mark";
import { Button } from "@/components/ui/Button";
import { t } from "@/lib/i18n";

export function EmptyState({
  onSignIn,
  onImport,
}: {
  onSignIn?: () => void;
  onImport?: () => void;
}) {
  return (
    <div className="grid h-full place-items-center px-8">
      <div className="flex max-w-[520px] flex-col items-center text-center">
        <span className="grid size-[72px] place-items-center rounded-[20px] bg-surface-3 text-text-1 shadow-[inset_0_0_0_1px_var(--hairline-strong),inset_0_1px_0_var(--sheen),var(--shadow-float)]">
          <Mark size={34} />
        </span>
        <h1 className="mt-6 text-28 leading-tight font-semibold tracking-[-0.02em] text-balance text-text-1">
          {t("empty.title")}
        </h1>
        <p className="mt-2.5 max-w-[42ch] text-14 text-text-2">{t("empty.body")}</p>
        <div className="mt-6 flex gap-3">
          <Button variant="primary" className="h-10 rounded-full px-5 text-14" onClick={onSignIn}>
            <LogIn size={16} strokeWidth={2} />
            {t("empty.signIn")}
          </Button>
          <Button className="h-10 rounded-full px-5 text-14" onClick={onImport}>
            <FolderInput size={16} strokeWidth={1.75} />
            {t("empty.import")}
          </Button>
        </div>
      </div>
    </div>
  );
}
