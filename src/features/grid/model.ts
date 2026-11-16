import type { GridLayout } from "@/lib/ipc";

/** Same layout by type + its own fields. Saved settings come back key-sorted and deep-merged
 * (stale keys of an earlier layout type), so JSON text never compares. */
export function sameLayout(a: GridLayout | undefined, b: GridLayout | undefined): boolean {
  if (!a || !b || a.type !== b.type) return false;
  switch (a.type) {
    case "columns":
      return a.count === (b as typeof a).count;
    case "rows": {
      const r = b as typeof a;
      return String(a.counts) === String(r.counts) && (a.align ?? "left") === (r.align ?? "left");
    }
    case "focus": {
      const f = b as typeof a;
      return a.main === f.main && a.others === f.others;
    }
    default:
      return true;
  }
}

/** Click to pick (appended), click again to unpick (later ones shift down). */
export const togglePick = (order: string[], id: string) =>
  order.includes(id) ? order.filter((x) => x !== id) : [...order, id];

/** Tile in fractions (0..1) of the thumbnail. `i` = slot index. */
export type Tile = { i: number; x: number; y: number; w: number; h: number };

const ASPECT = 1.7; // thumbnail w / h, close to a monitor

/** Rows of `counts` tiles, each row centred. */
function rows(counts: number[]): Tile[] {
  const cols = Math.max(...counts);
  const out: Tile[] = [];
  let i = 0;
  counts.forEach((c, r) => {
    for (let k = 0; k < c; k++)
      out.push({
        i: i++,
        x: (1 - c / cols) / 2 + k / cols,
        y: r / counts.length,
        w: 1 / cols,
        h: 1 / counts.length,
      });
  });
  return out;
}

/** `n` tiles over `cols` columns; the last row holds the rest. */
function grid(n: number, cols: number): Tile[] {
  const r = Math.ceil(n / cols);
  cols = Math.ceil(n / r); // 4 over 3 columns is 2 + 2, not 3 + 1
  const counts = Array.from({ length: r }, (_, k) => Math.min(cols, n - k * cols));
  return rows(counts);
}

/** Sub-grid of `k` tiles placed in the box `b`, slot indexes from `from`. */
function box(k: number, cols: number, b: Omit<Tile, "i">, from: number): Tile[] {
  return grid(k, cols).map((t) => ({
    i: from + t.i,
    x: b.x + t.x * b.w,
    y: b.y + t.y * b.h,
    w: t.w * b.w,
    h: t.h * b.h,
  }));
}

const cascade = (n: number): Tile[] => {
  if (n < 2) return [{ i: 0, x: 0, y: 0, w: 1, h: 1 }];
  const w = 0.56;
  const h = 0.52;
  return Array.from({ length: n }, (_, i) => ({
    i,
    x: (i * (1 - w)) / (n - 1),
    y: (i * (1 - h)) / (n - 1),
    w,
    h,
  }));
};

/** Thumbnail geometry for `n` windows. Approximates the engine; tile 0 is the main. */
export function tiles(layout: GridLayout, n: number): Tile[] {
  if (n < 1) return [];
  switch (layout.type) {
    case "single":
      return [{ i: 0, x: 0, y: 0, w: 1, h: 1 }];
    case "columns":
      return grid(n, Math.max(1, layout.count));
    case "rows":
      return rows(layout.counts.length ? layout.counts : [n]);
    case "pyramid": {
      const counts: number[] = [];
      for (let left = n, c = 1; left > 0; left -= c, c++) counts.push(Math.min(c, left));
      return rows(counts);
    }
    case "cascade":
      return cascade(n);
    case "wide": // main left, the rest stacked on the right
      return n < 2
        ? tiles({ type: "single" }, 1)
        : [{ i: 0, x: 0, y: 0, w: 0.6, h: 1 }, ...box(n - 1, 1, { x: 0.6, y: 0, w: 0.4, h: 1 }, 1)];
    case "focus": {
      const main = { i: 0, x: 0, y: 0, w: 1, h: 1 };
      const k = n - 1;
      if (k < 1 || layout.others === "minimize" || layout.others === "behind") return [main];
      if (layout.others === "side")
        return [
          { ...main, w: 0.55 },
          ...box(k, k > 3 ? 2 : 1, { x: 0.55, y: 0, w: 0.45, h: 1 }, 1),
        ];
      if (layout.others === "strip")
        return [{ ...main, h: 0.68 }, ...box(k, k, { x: 0, y: 0.68, w: 1, h: 0.32 }, 1)];
      // corner: small ones stacked on the main's bottom-right
      const w = 0.2;
      const h = Math.min(0.22, 0.9 / k);
      return [
        main,
        ...Array.from({ length: k }, (_, j) => ({ i: j + 1, x: 1 - w, y: 1 - (j + 1) * h, w, h })),
      ];
    }
    default: // auto, bands
      return grid(n, Math.ceil(Math.sqrt(n * ASPECT)));
  }
}
