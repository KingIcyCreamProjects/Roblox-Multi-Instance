import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { ChevronDown, ChevronRight, Globe, Link2, Play, Save, Trash2, Wrench } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { memo, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useSettings } from "@/app/useSettings";
import { useUi } from "@/app/store";
import { Button } from "@/components/ui/Button";
import { ServerPick } from "@/features/playbar/ServerPick";
import { Chip } from "@/features/playbar/DestChip";
import { targetFromBar, usePlayBar } from "@/features/playbar/store";
import { usePlay } from "@/features/playbar/usePlay";
import { runHere, sendTo, useElsewhere, useMoving, useTargets } from "@/features/devices/elsewhere";
import { toast } from "@/components/Toasts";
import {
  call,
  fileSrc,
  type AccountPatch,
  type AccountView,
  type AccountsList,
  type LaunchMode,
  type LaunchTarget,
} from "@/lib/ipc";
import { t, type MsgKey } from "@/lib/i18n";
import { Avatar, presenceDot, presenceKey } from "./AccountRow";
import { TAGS, content as menuContent, item as menuItem } from "./AccountMenu";
import { DestLabel, useDestName } from "./DestLabel";
import { ago, destText, label } from "./model";
import { openSite } from "./openSite";
import { namesHidden, useAccountActions } from "./useAccounts";

/** Side panel box: rem so it scales with the UI, capped so the list keeps room at small windows. */
export const PANEL_W = "w-[25rem] max-w-[42%] shrink-0 rounded-panel";

/** Right glass panel: the selected group's destination, one account's details, or a batch summary. */
export const Inspector = memo(function Inspector({ list }: { list: AccountsList }) {
  const ids = useUi((u) => u.sel.ids);
  const selGroup = useUi((u) => u.selGroup);
  const picked = list.accounts.filter((a) => ids.includes(a.id));
  // a header click selects the group; any other selection change clears selGroup
  const group =
    selGroup && picked.length && picked.every((a) => a.group === selGroup) ? selGroup : null;
  const key = group
    ? `group:${group}`
    : picked.length === 1
      ? picked[0].id
      : picked.length
        ? "multi"
        : "none";

  return (
    <aside
      aria-label={t("inspector.label")}
      className={clsx("glass-panel relative flex flex-col overflow-hidden", PANEL_W)}
    >
      {/* no exit/wait: arrowing through accounts swaps the panel at once instead of lagging behind */}
      <AnimatePresence initial={false}>
        <motion.div
          key={key}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.15 }}
          className="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]"
        >
          {group ? (
            <GroupPanel name={group} list={list} />
          ) : picked.length === 1 ? (
            <Single
              a={picked[0]}
              groups={list.groups}
              groupTarget={list.groupTargets[picked[0].group]}
            />
          ) : picked.length ? (
            <Multi picked={picked} groups={list.groups} groupTargets={list.groupTargets} />
          ) : (
            <p className="grid h-full place-items-center px-8 text-center text-13 text-text-3">
              {t("inspector.none")}
            </p>
          )}
        </motion.div>
      </AnimatePresence>
    </aside>
  );
});

const targetText = (d: LaunchTarget | null | undefined) =>
  d ? (d.privateLink ?? (d.placeId ? String(d.placeId) : "")) : "";

const divider = <div className="h-px bg-hairline" />;

/** Destination field + Save (+ Clear). The text becomes a LaunchTarget through the Play bar's parser. */
function DestEditor({
  initial,
  hint,
  saved,
  inherited,
  onSave,
  onClear,
}: {
  initial: string;
  hint: ReactNode;
  /** shown under the buttons: what is saved right now */
  saved?: LaunchTarget | null;
  /** nothing saved here: the group destination it uses instead */
  inherited?: { label: string; d: LaunchTarget };
  onSave: (target: LaunchTarget) => Promise<boolean>;
  onClear?: () => Promise<boolean>;
}) {
  const id = useId();
  const [text, setText] = useState(initial);
  const [bad, setBad] = useState<MsgKey | null>(null);
  const [mode, setMode] = useState<LaunchMode>(saved?.mode ?? (saved?.jobId ? "specific" : "any"));
  const [job, setJob] = useState(saved?.jobId ?? "");
  // saved value changed (our Save, Clear, or elsewhere): resync in place, no remount (keeps focus)
  // keyed on the whole saved target: a new mode or JobId on the same place resyncs too
  const savedKey = `${initial}\0${JSON.stringify(saved ?? null)}`;
  const [prev, setPrev] = useState(savedKey);
  if (savedKey !== prev) {
    setPrev(savedKey);
    setText(initial);
    setMode(saved?.mode ?? (saved?.jobId ? "specific" : "any"));
    setJob(saved?.jobId ?? "");
  }
  const place = targetFromBar(text, "", "any");
  // the server pick only applies to a game (not a private / share link)
  const showMode = !!place?.placeId && !place.privateLink;
  const parsed = targetFromBar(text, job, showMode ? mode : "any");
  // nothing typed, or still what was loaded: Save would only error or rewrite the same value
  const dirty =
    text.trim() !== initial ||
    (showMode &&
      (mode !== (saved?.mode ?? (saved?.jobId ? "specific" : "any")) ||
        job.trim() !== (saved?.jobId ?? "")));
  const submit = () => {
    if (!text.trim() || !dirty) return;
    if (!place) return setBad("dest.invalid");
    if (!parsed || (showMode && mode === "specific" && !parsed.jobId))
      return setBad("play.noServer");
    setBad(null);
    void onSave(parsed);
  };
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-2">
        <FieldLabel htmlFor={id}>{t("dest.label")}</FieldLabel>
        <div
          className={clsx(
            "flex h-9 items-center gap-2 rounded-control bg-field px-3 shadow-[inset_0_0_0_1px_var(--hairline-strong)] focus-within:shadow-[var(--focus-ring)]",
            bad === "dest.invalid" && "shadow-[inset_0_0_0_1px_var(--status-error)]",
          )}
        >
          <Link2 size={15} strokeWidth={1.75} aria-hidden className="shrink-0 text-text-2" />
          <input
            id={id}
            value={text}
            spellCheck={false}
            placeholder={t("dest.placeholder")}
            aria-invalid={bad === "dest.invalid"}
            onChange={(e) => {
              setText(e.target.value);
              setBad(null);
            }}
            onKeyDown={(e) => e.key === "Enter" && submit()}
            className="min-w-0 flex-1 bg-transparent text-13 text-text-1 placeholder:text-text-3 focus:shadow-none focus:outline-none"
          />
        </div>
        {bad ? (
          <p role="alert" className="text-12 leading-snug text-error">
            {t(bad)}
          </p>
        ) : (
          <p className="text-12 leading-snug text-text-3">{hint}</p>
        )}
        {parsed && !bad && text.trim() !== targetText(saved) && (
          <div className="min-h-5 text-text-2">
            <Chip id={`${id}-pv`} input={text} />
          </div>
        )}
      </div>
      {showMode && (
        <div className="flex flex-col gap-2">
          <FieldLabel>{t("mode.label")}</FieldLabel>
          <ServerPick
            mode={mode}
            job={job}
            onMode={(m) => {
              setMode(m);
              setBad(null);
            }}
            onJob={(j) => {
              setJob(j);
              setBad(null);
            }}
            onEnter={submit}
          />
        </div>
      )}
      <div className="flex gap-2">
        <Button variant="primary" disabled={!text.trim() || !dirty} onClick={submit}>
          <Save size={15} strokeWidth={1.75} />
          {t("dest.save")}
        </Button>
        {onClear && (
          <Button
            onClick={() =>
              // a failed write keeps the field on what is still saved
              void onClear().then((ok) => {
                if (!ok) return;
                setText("");
                setMode("any");
                setJob("");
                setBad(null);
              })
            }
          >
            {t("dest.clear")}
          </Button>
        )}
      </div>
      {(saved || inherited) && (
        <div className="flex items-baseline gap-2 text-12 text-text-3">
          <span data-pii={saved ? undefined : true} className="shrink-0">
            {saved ? t("dest.saved") : inherited!.label}
          </span>
          <span className="min-w-0 truncate font-medium text-text-2">
            <DestLabel d={saved ?? inherited!.d} />
          </span>
        </div>
      )}
    </div>
  );
}

/** A group header was clicked: its accounts, and one destination saved to the group and each account. */
function GroupPanel({ name, list }: { name: string; list: AccountsList }) {
  const qc = useQueryClient();
  const act = useAccountActions();
  const members = list.accounts
    .filter((a) => a.group === name)
    .sort((x, y) => Number(y.pinned) - Number(x.pinned) || x.order - y.order);
  const ids = members.map((a) => a.id);
  const saved = list.groupTargets[name] ?? null;
  // every member's own (one atomic write), then the group's; a failed group write puts the
  // members' old targets back, so the two never disagree
  const apply = async (target: LaunchTarget | null) => {
    const before = new Map<string, string[]>();
    for (const a of members) {
      const k = JSON.stringify(a.launchTarget ?? null);
      before.set(k, [...(before.get(k) ?? []), a.id]);
    }
    let ok = await act.updateMany(ids, { launchTarget: target });
    if (ok && !(await act.setGroupTarget(name, target))) {
      ok = false;
      for (const [k, back] of before)
        await act.updateMany(back, { launchTarget: JSON.parse(k) as LaunchTarget | null });
    }
    if (!ok) toast(t("dest.saveFailed"));
    else if (namesHidden(qc))
      toast(t(target ? "dest.savedGroupAnon" : "dest.clearedGroupAnon", { n: ids.length }));
    else toast(t(target ? "dest.savedGroup" : "dest.clearedGroup", { name, n: ids.length }));
    return ok;
  };
  return (
    <div className="flex flex-col gap-4 p-[1.375rem]">
      <div>
        <h2 data-pii className="truncate text-20 leading-tight font-semibold text-text-1">
          {name}
        </h2>
        <p className="mt-0.5 text-12 text-text-3">{t("group.label")}</p>
      </div>
      <ul className="flex max-h-44 flex-col gap-0.5 overflow-y-auto">
        {members.map((a) => (
          <li key={a.id} className="flex h-[1.625rem] items-center gap-2.5 text-13 text-text-2">
            <span className="relative shrink-0">
              <Avatar a={a} size={20} round />
              <span
                aria-hidden
                className={clsx(
                  "absolute -right-px -bottom-px size-1.5 rounded-full shadow-[0_0_0_1.5px_var(--glass)]",
                  presenceDot[a.presence?.state ?? "offline"],
                )}
              />
            </span>
            <span data-pii className="truncate">
              {label(a)}
            </span>
          </li>
        ))}
      </ul>
      {divider}
      <DestEditor
        key={name}
        initial={targetText(saved)}
        hint={t("dest.hintGroup", { n: ids.length })}
        saved={saved}
        onSave={apply}
        onClear={saved ? () => apply(null) : undefined}
      />
    </div>
  );
}

/** Average colour of an image for the bloom; null when unreadable (tainted canvas, no image). */
export function useBloom(src: string | null) {
  const [c, setC] = useState<string | null>(null);
  useEffect(() => {
    setC(null);
    if (!src) return;
    let dead = false;
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => {
      if (dead) return;
      try {
        const cv = document.createElement("canvas");
        cv.width = cv.height = 16;
        const g = cv.getContext("2d")!;
        g.drawImage(img, 0, 0, 16, 16);
        const d = g.getImageData(0, 0, 16, 16).data;
        let [r, gr, b, n] = [0, 0, 0, 0];
        for (let i = 0; i < d.length; i += 4)
          if (d[i + 3] > 40) [r, gr, b, n] = [r + d[i], gr + d[i + 1], b + d[i + 2], n + 1];
        setC(n ? `rgb(${(r / n) | 0} ${(gr / n) | 0} ${(b / n) | 0})` : null);
      } catch {
        setC(null);
      }
    };
    img.src = src;
    return () => {
      dead = true;
    };
  }, [src]);
  return c;
}

function Hero({ a }: { a: AccountView }) {
  const { data: path } = useQuery({
    queryKey: ["avatar_full", a.userId],
    queryFn: () => call("avatar_full", { userId: a.userId! }),
    enabled: a.userId !== null,
    meta: { silent: true },
  });
  const src = fileSrc(path ?? null);
  const bloom = useBloom(src);
  return (
    <div className="relative -mx-2 grid h-[10.625rem] place-items-center overflow-hidden">
      <div
        aria-hidden
        className="absolute top-1/2 left-1/2 size-[12.5rem] -translate-x-1/2 -translate-y-1/2 rounded-full opacity-45 blur-[56px]"
        style={{
          background: `radial-gradient(closest-side, ${bloom ?? "color-mix(in oklch, var(--accent) 22%, var(--surface-4))"}, transparent)`,
        }}
      />
      {src ? (
        <img
          data-pii
          src={src}
          alt=""
          draggable={false}
          className="relative h-[9.375rem] w-auto object-contain"
        />
      ) : (
        <Avatar a={a} size={112} round className="relative" />
      )}
    </div>
  );
}

function FieldLabel({ htmlFor, children }: { htmlFor?: string; children: ReactNode }) {
  return (
    <label htmlFor={htmlFor} className="text-12 font-medium text-text-2">
      {children}
    </label>
  );
}

const inputCls =
  "w-full rounded-control border border-transparent bg-surface-3 px-2.5 text-13 text-text-1 placeholder:text-text-3 transition-colors hover:bg-surface-4 focus:border-hairline-strong";

/** Text that saves on blur or Enter; Esc reverts. */
function EditText({
  label: lbl,
  value,
  max,
  placeholder,
  multiline,
  focusOn,
  owner,
  pii,
  onCommit,
}: {
  label: string;
  value: string;
  max: number;
  placeholder?: string;
  multiline?: boolean;
  focusOn?: "alias" | "note";
  owner?: string;
  pii?: boolean;
  onCommit: (v: string) => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState<string | null>(null);
  const ref = useRef<HTMLInputElement & HTMLTextAreaElement>(null);
  const req = useUi((u) => u.focusReq);
  useEffect(() => {
    if (!focusOn || req?.field !== focusOn || req.owner !== owner) return;
    ref.current?.focus();
    ref.current?.select();
    useUi.setState({ focusReq: null });
  }, [req, focusOn, owner]);
  // Esc: the blur that follows must not save (set synchronously, no render-order reliance)
  const cancelled = useRef(false);
  const commit = () => {
    if (!cancelled.current && draft !== null && draft !== value) onCommit(draft);
    cancelled.current = false;
    setDraft(null);
  };
  const props = {
    id,
    ref,
    value: draft ?? value,
    maxLength: max,
    placeholder,
    "data-pii": pii || undefined,
    onChange: (e: React.ChangeEvent<HTMLInputElement & HTMLTextAreaElement>) =>
      setDraft(e.target.value),
    onBlur: commit,
    onKeyDown: (e: React.KeyboardEvent<HTMLElement>) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        cancelled.current = true;
        ref.current?.blur();
      } else if (e.key === "Enter" && (!multiline || e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        ref.current?.blur();
      }
    },
  };
  return (
    <div className="flex flex-col gap-1.5">
      <FieldLabel htmlFor={id}>{lbl}</FieldLabel>
      {multiline ? (
        <textarea {...props} rows={3} className={clsx(inputCls, "resize-y py-2 leading-snug")} />
      ) : (
        <input {...props} className={clsx(inputCls, "h-8")} />
      )}
      {draft !== null && draft.length > max * 0.9 && (
        <p className="text-12 text-text-3 tnum">{t("field.left", { n: max - draft.length })}</p>
      )}
    </div>
  );
}

function GroupPicker({
  owner,
  value,
  groups,
  onPick,
}: {
  owner: string;
  value: string | null;
  groups: string[];
  onPick: (g: string) => void;
}) {
  const id = useId();
  const req = useUi((u) => u.focusReq);
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  // Enter/Esc then the unmount blur: only the first one counts
  const settled = useRef(false);
  const startAdding = () => {
    settled.current = false;
    setAdding(true);
  };
  useEffect(() => {
    if (req?.field !== "newGroup" || req.owner !== owner) return;
    settled.current = false;
    setAdding(true);
    useUi.setState({ focusReq: null });
  }, [req, owner]);
  const done = (save: boolean) => {
    if (settled.current) return;
    settled.current = true;
    const n = name.trim();
    if (save && n) onPick(n);
    setAdding(false);
    setName("");
  };
  return (
    <div className="flex flex-col gap-1.5">
      <FieldLabel htmlFor={id}>{t("inspector.group")}</FieldLabel>
      {adding ? (
        <input
          id={id}
          autoFocus
          data-pii
          maxLength={50}
          value={name}
          placeholder={t("inspector.newGroupName")}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => done(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter") done(true);
            if (e.key === "Escape") {
              e.stopPropagation();
              done(false);
            }
          }}
          className={clsx(inputCls, "h-8")}
        />
      ) : (
        <select
          id={id}
          data-pii
          value={value ?? ""}
          onChange={(e) => (e.target.value === "\0new" ? startAdding() : onPick(e.target.value))}
          className={clsx(inputCls, "h-8")}
        >
          {value === null && <option value="">{t("inspector.mixed")}</option>}
          {groups.map((g) => (
            <option key={g} value={g}>
              {g}
            </option>
          ))}
          <option value={"\0new"}>{t("menu.newGroup")}</option>
        </select>
      )}
    </div>
  );
}

function Swatches({
  value,
  onPick,
}: {
  value: number | null | undefined;
  onPick: (c: number | null) => void;
}) {
  const name = useId();
  const opts: [number | null, string, string][] = [
    [null, t("color.none"), "transparent"],
    ...TAGS.map((k, i): [number, string, string] => [i + 1, t(k), `var(--tag-${i + 1})`]),
  ];
  return (
    <fieldset className="flex flex-col gap-1.5">
      <legend className="mb-1.5 text-12 font-medium text-text-2">{t("inspector.color")}</legend>
      <div className="flex flex-wrap gap-1.5">
        {opts.map(([c, lbl, bg]) => (
          <label
            key={lbl}
            title={lbl}
            className="grid size-7 cursor-pointer place-items-center rounded-full has-[:checked]:ring-2 has-[:checked]:ring-text-2 has-[:focus-visible]:shadow-[var(--focus-ring)]"
          >
            <input
              type="radio"
              name={name}
              className="sr-only"
              aria-label={lbl}
              checked={value === c}
              onChange={() => onPick(c)}
            />
            <span
              aria-hidden
              className={"size-5 rounded-full border border-hairline-strong"}
              style={{ background: bg }}
            />
          </label>
        ))}
      </div>
    </fieldset>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <h3 className="text-12 font-medium text-text-2">{title}</h3>
      {children}
    </div>
  );
}

/** Optional whole number from 1 to `max`; empty = the global setting. */
function NumField({
  value,
  max,
  label,
  err,
  onCommit,
}: {
  value: number | null;
  max: number;
  label: MsgKey;
  err: MsgKey;
  onCommit: (v: number | null) => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState<string | null>(null);
  const shown = draft ?? (value === null ? "" : String(value));
  const n = Number(shown.trim());
  const bad = draft !== null && draft.trim() !== "" && (!Number.isInteger(n) || n < 1 || n > max);
  return (
    <div className="flex flex-col gap-1.5">
      <FieldLabel htmlFor={id}>{t(label)}</FieldLabel>
      <input
        id={id}
        type="text"
        inputMode="numeric"
        value={shown}
        placeholder={t("inspector.fpsGlobal")}
        aria-invalid={bad}
        aria-describedby={bad ? `${id}-err` : undefined}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          // invalid input reverts to the saved value
          if (draft !== null && !bad) onCommit(draft.trim() === "" ? null : n);
          setDraft(null);
        }}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        className={clsx(inputCls, "h-8 w-32 tnum", bad && "border-error")}
      />
      {bad && (
        <p id={`${id}-err`} role="alert" className="text-12 text-error">
          {t(err)}
        </p>
      )}
    </div>
  );
}

function ExtraField({
  value,
  onCommit,
}: {
  value: Record<string, string>;
  onCommit: (v: Record<string, string>) => void;
}) {
  const text = Object.entries(value)
    .map(([k, v]) => `${k}=${v}`)
    .join("\n");
  return (
    <EditText
      label={t("inspector.extra")}
      value={text}
      max={5000}
      multiline
      placeholder={t("inspector.extraPlaceholder")}
      onCommit={(s) =>
        onCommit(
          Object.fromEntries(
            s
              .split("\n")
              .map((l) => l.split(/=(.*)/s).map((x) => x.trim()))
              .filter(([k]) => k)
              .map(([k, v = ""]) => [k, v]),
          ),
        )
      }
    />
  );
}

function sessionText(a: AccountView) {
  const when = a.lastValidatedAt
    ? ` · ${t("session.checked", { when: ago(a.lastValidatedAt) })}`
    : "";
  switch (a.session) {
    case "ok":
      return t("session.ok") + when;
    case "expired":
      return t("session.expiredLong");
    case "pending":
      return t("session.pendingLong");
    default:
      return t("session.unknown");
  }
}

/** "Run on" button: the paired computers at least one of these accounts is not running on yet. */
function RunOn({ accounts }: { accounts: AccountView[] }) {
  const where = useElsewhere();
  const places = useTargets().filter((p) =>
    accounts.some((a) => a.userId !== null && where.get(a.userId)?.ip !== p.ip),
  );
  const moving = useMoving((m) => accounts.some((a) => m.ids.includes(a.id)));
  // a Run here button sits beside it and already says "Moving…": one indicator, not two
  const besideRunHere = accounts.some((a) => a.userId !== null && where.has(a.userId));
  if (!places.length) return null;
  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild disabled={moving}>
        <Button>
          {t(moving && !besideRunHere ? "elsewhere.moving" : "elsewhere.runOn")}
          <ChevronDown size={14} strokeWidth={1.75} aria-hidden />
        </Button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className={menuContent}
          align="start"
          sideOffset={4}
          collisionPadding={8}
        >
          {places.map((p) => (
            <DropdownMenu.Item
              key={p.ip}
              className={menuItem}
              onSelect={() => void sendTo(accounts, p)}
            >
              <span className="flex-1 truncate">{p.device}</span>
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function Single({
  a,
  groups,
  groupTarget,
}: {
  a: AccountView;
  groups: string[];
  groupTarget: LaunchTarget | undefined;
}) {
  const act = useAccountActions();
  const { play } = usePlay();
  const { data: s } = useSettings();
  const { data: caps } = useQuery({
    queryKey: ["caps"],
    queryFn: () => call("platform_capabilities"),
  });
  // no FPS / graphics settings file to write on this OS (same gate as the Settings rows)
  const clientHelpers = caps?.clientHelpers !== false;
  const focusReq = useUi((u) => u.focusReq);
  const barDest = usePlayBar((b) => !!b.destination.trim());
  const save = (patch: AccountPatch) => void act.update(a.id, patch);
  const state = a.presence?.state ?? "offline";
  const game = state === "in_game" ? a.presence?.gameName : null;
  const where = useElsewhere().get(a.userId ?? NaN);
  const moving = useMoving((m) => m.ids.includes(a.id));
  const own = a.launchTarget;
  const fromGroup = destText(groupTarget, useDestName(groupTarget));
  // F2 / "Edit alias" from the list: open More so the field exists to be focused
  // sticky: the field clears focusReq once focused, More must stay open after that
  const [more, setMore] = useState(false);
  if (focusReq?.owner === a.id && !more) setMore(true);
  const showMore = more || focusReq?.owner === a.id;
  const apply = async (launchTarget: LaunchTarget | null) => {
    const ok = await act.update(a.id, { launchTarget });
    if (ok) toast(t(launchTarget ? "menu.saveDestDone" : "dest.cleared"));
    return ok;
  };

  return (
    <div className="flex flex-col gap-4 p-[1.375rem]">
      <div>
        <h2 data-pii className="truncate text-20 leading-tight font-semibold text-text-1">
          {label(a)}
        </h2>
        <p className="mt-0.5 flex items-center gap-1.5 text-12 text-text-3">
          <span
            aria-hidden
            className={clsx("size-[0.4375rem] shrink-0 rounded-full", presenceDot[state])}
          />
          <span className="truncate">
            {t(presenceKey[state])}
            {game ? ` · ${game}` : ""}
          </span>
        </p>
        {where && (
          <p className="mt-0.5 truncate text-12 text-text-3">
            {t("elsewhere.runningOn", { device: where.device })}
          </p>
        )}
      </div>
      {divider}
      <DestEditor
        key={a.id}
        initial={targetText(own)}
        hint={
          !groupTarget ? (
            t("dest.hintAccount")
          ) : own ? (
            <span data-pii>{t("dest.hintOverride", { name: a.group, dest: fromGroup ?? "" })}</span>
          ) : (
            <span data-pii>{t("dest.hintWillOverride", { name: a.group })}</span>
          )
        }
        saved={own}
        inherited={
          groupTarget
            ? { label: t("dest.fromGroup", { name: a.group }), d: groupTarget }
            : undefined
        }
        onSave={apply}
        onClear={own ? () => apply(null) : undefined}
      />
      {divider}
      <button
        type="button"
        aria-expanded={showMore}
        onClick={() => setMore(!showMore)}
        className="flex h-8 items-center gap-1.5 self-start rounded-control px-2 text-12 font-semibold text-text-2 hover:bg-surface-3 hover:text-text-1"
      >
        <ChevronRight
          size={14}
          strokeWidth={2}
          aria-hidden
          className={clsx(
            "transition-transform duration-[var(--dur-quick)]",
            showMore && "rotate-90",
          )}
        />
        {t("inspector.more")}
      </button>
      {showMore && (
        <div className="flex flex-col gap-4">
          <Hero a={a} />
          <div className="flex flex-wrap gap-2">
            <Button
              variant="primary"
              disabled={a.session === "pending"}
              onClick={() => play([a.id])}
            >
              <Play size={14} strokeWidth={2.25} fill="currentColor" />
              {/* same words as the Play bar: no game anywhere opens Roblox's home screen */}
              {t(own || groupTarget || barDest ? "menu.play" : "playbar.home")}
            </Button>
            {where && (
              <Button disabled={moving} onClick={() => void runHere([a])}>
                {t(moving ? "elsewhere.moving" : "elsewhere.runHere")}
              </Button>
            )}
            <RunOn accounts={[a]} />
            <Button onClick={() => openSite(a.id)}>
              <Globe size={14} strokeWidth={1.75} />
              {t("inspector.openSite")}
            </Button>
            <Button onClick={() => useUi.setState({ toolsOpen: true })}>
              <Wrench size={14} strokeWidth={1.75} />
              {t("menu.tools")}
            </Button>
          </div>
          <p data-pii className="truncate text-12 text-text-3">
            @{a.username}
          </p>
          <EditText
            label={t("inspector.alias")}
            value={a.alias ?? ""}
            max={50}
            placeholder={a.displayName}
            focusOn="alias"
            owner={a.id}
            pii
            onCommit={(v) => save({ alias: v.trim() || null })}
          />
          <EditText
            label={t("inspector.note")}
            value={a.note ?? ""}
            max={5000}
            multiline
            focusOn="note"
            owner={a.id}
            placeholder={t("inspector.notePlaceholder")}
            onCommit={(v) => save({ note: v.trim() ? v : null })}
          />
          <GroupPicker
            owner={a.id}
            value={a.group}
            groups={groups}
            onPick={(group) => save({ group })}
          />
          <Swatches value={a.color} onPick={(color) => save({ color })} />
          {clientHelpers && (
            <>
              <NumField
                value={a.fpsCap}
                max={240}
                label="inspector.fps"
                err="inspector.fpsErr"
                onCommit={(fpsCap) => save({ fpsCap })}
              />
              <NumField
                value={a.graphics}
                max={10}
                label="inspector.graphics"
                err="inspector.graphicsErr"
                onCommit={(graphics) => save({ graphics })}
              />
            </>
          )}
          <Section title={t("inspector.session")}>
            <div className="flex items-center gap-2">
              <p
                className={clsx(
                  "flex-1 text-13",
                  a.session === "expired" ? "text-warn" : "text-text-1",
                )}
              >
                {sessionText(a)}
              </p>
              {a.session === "ok" || a.session === "unknown" ? (
                <Button variant="ghost" onClick={() => void act.validate([a.id])}>
                  {t("session.validate")}
                </Button>
              ) : (
                <Button onClick={() => void act.signIn(a.id, a.username)}>
                  {t("menu.signInAgain")}
                </Button>
              )}
            </div>
          </Section>
          {s?.dev.enabled && <ExtraField value={a.extra} onCommit={(extra) => save({ extra })} />}
        </div>
      )}
    </div>
  );
}

function Multi({
  picked,
  groups,
  groupTargets,
}: {
  picked: AccountView[];
  groups: string[];
  groupTargets: AccountsList["groupTargets"];
}) {
  const act = useAccountActions();
  const { play } = usePlay();
  const barDest = usePlayBar((b) => !!b.destination.trim());
  const home = !barDest && !picked.some((a) => a.launchTarget || groupTargets[a.group]);
  const ids = picked.map((a) => a.id);
  const sameGroup = picked.every((a) => a.group === picked[0].group) ? picked[0].group : null;
  const sameColor = picked.every((a) => a.color === picked[0].color) ? picked[0].color : undefined;
  const where = useElsewhere();
  const away = picked.filter((a) => a.userId !== null && where.has(a.userId));
  // every picked account, the same scope as Run on: a send of any of them disables both
  const moving = useMoving((m) => picked.some((a) => m.ids.includes(a.id)));
  return (
    <div className="flex flex-col gap-5 p-[1.375rem]">
      <div className="flex items-center pl-3">
        {picked.slice(0, 6).map((a) => (
          <Avatar
            key={a.id}
            a={a}
            size={56}
            round
            className="-ml-3 ring-3 ring-[var(--glass-panel)]"
          />
        ))}
        {picked.length > 6 && (
          <span className="-ml-3 grid size-14 place-items-center rounded-full bg-surface-3 text-13 font-semibold text-text-2 ring-3 ring-[var(--glass-panel)] tnum">
            +{picked.length - 6}
          </span>
        )}
      </div>
      <h2 className="text-20 leading-tight font-semibold text-text-1 tnum">
        {t("inspector.selected", { n: picked.length })}
      </h2>
      <div className="flex flex-wrap gap-2">
        <Button variant="primary" onClick={() => play(ids)}>
          <Play size={14} strokeWidth={2.25} fill="currentColor" />
          {home
            ? t("playbar.homeN", { n: picked.length })
            : t("playbar.playN", { n: picked.length })}
        </Button>
        {away.length > 0 && (
          <Button disabled={moving} onClick={() => void runHere(away)}>
            {moving ? t("elsewhere.moving") : t("elsewhere.runNHere", { n: away.length })}
          </Button>
        )}
        <RunOn accounts={picked} />
        <Button onClick={() => useUi.setState({ toolsOpen: true })}>
          <Wrench size={14} strokeWidth={1.75} />
          {t("menu.tools")}
        </Button>
        <Button variant="danger" onClick={() => void act.remove(ids)}>
          <Trash2 size={14} strokeWidth={1.75} />
          {t("menu.removeN", { n: picked.length })}
        </Button>
      </div>
      <div className="flex flex-col gap-4 border-t border-hairline pt-5">
        <GroupPicker
          owner="multi"
          value={sameGroup}
          groups={groups}
          onPick={(group) => void act.updateMany(ids, { group })}
        />
        <Swatches value={sameColor} onPick={(color) => void act.updateMany(ids, { color })} />
      </div>
    </div>
  );
}
