//! Grid Arrange layout engine: pure geometry, no OS calls. `plan` gives each window a cell.
//! Only big / full-screen windows take their cell's size; the rest shrink to Roblox's minimum
//! (800x600 client), so a window may come out bigger or smaller than its cell and
//! `Target::pos` then places it by its real size (edges flush, neighbors overlap or gap).

use crate::accounts::Rect;
use serde::{Deserialize, Serialize};

/// How short rows sit: cells of the widest row's width, left or centered; or stretched.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    Left,
    #[default]
    Center,
    Stretch,
}

/// Where the non-focus windows go.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Others {
    /// Bottom strip under the big window.
    Strip,
    /// Column right of the big window.
    Side,
    /// Small, bottom-right corner over the big window.
    Corner,
    Minimize,
    /// Same rect as the big window, stacked under it.
    Behind,
}

/// Mockup presets: Classic 3-wide = `columns {count: 3}`, Even grid = `auto`, Bands,
/// Pyramid, Wide + stack = `wide`, Cascade, Focus + strip / side = `focus` with `others`
/// strip / side. `suggest` offers only those two; corner / minimize / behind are API-only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Layout {
    /// Balanced rows by count (4 = 2x2, 5 = 3+2, 7 = 4+3...), short row centered.
    Auto,
    /// `count` per row, left aligned, rows at least half the height (today's Grid Arrange).
    Columns { count: u32 },
    /// Explicit per-row counts; extra windows repeat the last count, missing ones drop rows.
    Rows {
        counts: Vec<u32>,
        #[serde(default)]
        align: Align,
    },
    /// Two rows, both stretched to the full width.
    Bands,
    /// 1, 2, 3... per row, centered.
    Pyramid,
    /// First window 60% wide on the left, the rest tiled on the right.
    Wide,
    /// Overlapped diagonally, title bars visible.
    Cascade,
    /// Everyone on the full area, first on top.
    Single,
    /// `main` (account id) big, the rest per `others`.
    Focus { main: String, others: Others },
}

impl Default for Layout {
    fn default() -> Self {
        Layout::Columns { count: 3 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    /// Wanted visible rect; the window may end up bigger.
    pub cell: Rect,
    /// Area the cell tiles; a bigger window keeps its relative spot in here.
    pub region: Rect,
    /// Stacking: higher is in front.
    pub z: i64,
    pub minimize: bool,
    /// Not a big / full-screen window: sized to Roblox's minimum (less to render), the cell
    /// only says where it goes.
    pub small: bool,
}

impl Target {
    /// Size to ask the OS for; Roblox grows a too-small ask to its minimum.
    pub fn size(&self) -> (i32, i32) {
        if self.small {
            (1, 1)
        } else {
            (self.cell.w, self.cell.h)
        }
    }

    /// Visible top-left for a window whose visible size came out as `(w, h)`: where the cell
    /// sits in its region, scaled to the leftover room, kept on `work`.
    pub fn pos(&self, work: Rect, (w, h): (i32, i32)) -> (i32, i32) {
        fn axis(c: i32, cs: i32, r: i32, rs: i32, s: i32, lo: i32, ls: i32) -> i32 {
            let p = if rs > cs {
                r + (i64::from(rs - s) * i64::from(c - r) / i64::from(rs - cs)) as i32
            } else {
                c
            };
            p.min(lo + ls - s).max(lo)
        }
        let (c, r) = (self.cell, self.region);
        (
            axis(c.x, c.w, r.x, r.w, w, work.x, work.w),
            axis(c.y, c.h, r.y, r.h, h, work.y, work.h),
        )
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

/// Sub-rect of `work` in per-mille fractions.
fn part(work: Rect, x: i32, y: i32, w: i32, h: i32) -> Rect {
    rect(
        work.x + work.w * x / 1000,
        work.y + work.h * y / 1000,
        work.w * w / 1000,
        work.h * h / 1000,
    )
}

fn chunk(n: usize, per: usize) -> Vec<usize> {
    (0..n.div_ceil(per)).map(|r| per.min(n - r * per)).collect()
}

/// Auto row counts for `n` windows.
pub fn auto_rows(n: usize) -> Vec<usize> {
    match n {
        0 => vec![],
        1..=3 => vec![n],
        4 => vec![2, 2],
        5 => vec![3, 2],
        6 => vec![3, 3],
        7 => vec![4, 3],
        8 => vec![4, 4],
        9 => vec![3, 3, 3],
        _ => chunk(n, n.isqrt() + usize::from(n.isqrt().pow(2) < n)),
    }
}

/// Fit explicit counts to `n`: drop zeros, cut at `n`, repeat the last count for leftovers.
fn fit_counts(counts: &[u32], n: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut left = n;
    let mut last = 0;
    for &c in counts.iter().filter(|&&c| c > 0) {
        if left == 0 {
            break;
        }
        last = (c as usize).min(left);
        out.push(last);
        left -= last;
    }
    if last == 0 {
        return auto_rows(n);
    }
    out.extend(chunk(left, last));
    out
}

/// Grid cells, row by row, filling `a`. Unstretched cells are `a.w / cols` wide (`cols` 0:
/// the widest row); rows are at least `a.h / min_rows` tall.
fn rows(counts: &[usize], align: Align, a: Rect, cols: usize, min_rows: usize) -> Vec<Target> {
    let r = counts.len().max(min_rows).max(1) as i32;
    let cols = if cols > 0 {
        cols
    } else {
        counts.iter().copied().max().unwrap_or(1)
    } as i32;
    let mut out = Vec::new();
    for (row, &k) in counts.iter().enumerate() {
        let (k, row) = (k as i32, row as i32);
        // Edges by proportion, so the last row ends exactly on the bottom.
        let (y0, y1) = (a.y + a.h * row / r, a.y + a.h * (row + 1) / r);
        let w = a.w / if align == Align::Stretch { k } else { cols };
        let x0 = a.x
            + if align == Align::Center {
                (a.w - k * w) / 2
            } else {
                0
            };
        for c in 0..k {
            out.push(cell(rect(x0 + c * w, y0, w, y1 - y0), a));
        }
    }
    out
}

/// A placed cell; middle columns over their neighbors, lower rows over upper ones.
fn cell(c: Rect, region: Rect) -> Target {
    let dx = (2 * c.x + c.w - 2 * region.x - region.w).abs() / 2;
    Target {
        cell: c,
        region,
        z: i64::from(c.y) - i64::from(dx) * 100_000,
        minimize: false,
        small: true,
    }
}

/// Target for each of `n` windows (in order) on `work`. `main` is the focus window's index.
pub fn plan(n: usize, work: Rect, layout: &Layout, main: usize) -> Vec<Target> {
    let main = main.min(n.saturating_sub(1));
    let full = |z| Target {
        cell: work,
        region: work,
        z,
        minimize: false,
        small: false,
    };
    match layout {
        Layout::Auto => rows(&auto_rows(n), Align::Center, work, 0, 1),
        Layout::Columns { count } => {
            // From settings or IPC: a huge count would wrap through `as i32` in `rows`.
            let c = (*count).clamp(1, 100) as usize;
            rows(&chunk(n, c), Align::Left, work, c, 2)
        }
        Layout::Rows { counts, align } => rows(&fit_counts(counts, n), *align, work, 0, 1),
        Layout::Bands => {
            let c: Vec<usize> = [n.div_ceil(2), n / 2]
                .into_iter()
                .filter(|&k| k > 0)
                .collect();
            rows(&c, Align::Stretch, work, 0, 1)
        }
        Layout::Pyramid => {
            let mut c = Vec::new();
            let mut left = n;
            while left > 0 {
                c.push((c.len() + 1).min(left));
                left -= c[c.len() - 1];
            }
            rows(&c, Align::Center, work, 0, 1)
        }
        Layout::Single => (0..n).map(|i| full(-(i as i64))).collect(),
        Layout::Cascade => {
            let (w, h) = (work.w * 58 / 100, work.h * 60 / 100);
            let steps = (n.max(2) - 1) as i32;
            (0..n as i32)
                .map(|i| Target {
                    cell: rect(
                        work.x + i * (work.w - w) / steps,
                        work.y + i * (work.h - h) / steps,
                        w,
                        h,
                    ),
                    region: work,
                    z: i64::from(i),
                    minimize: false,
                    small: true,
                })
                .collect()
        }
        Layout::Wide if n >= 2 => {
            let m = n - 1;
            let side = part(work, 600, 0, 400, 1000);
            let mut t = vec![Target {
                cell: part(work, 0, 0, 600, 1000),
                ..full(i64::MAX)
            }];
            t.extend(rows(
                &chunk(m, if m > 3 { 2 } else { 1 }),
                Align::Stretch,
                side,
                0,
                1,
            ));
            t
        }
        Layout::Focus { others, .. } if n >= 2 => {
            let m = n - 1;
            let (big, rest): (Rect, Vec<Target>) = match others {
                Others::Strip => {
                    let strip = part(work, 0, 740, 1000, 260);
                    let strip = rect(strip.x, strip.y, strip.w, work.y + work.h - strip.y);
                    (
                        part(work, 80, 0, 840, 720),
                        rows(&[m], Align::Stretch, strip, 0, 1),
                    )
                }
                Others::Side => {
                    let col = part(work, 720, 0, 280, 1000);
                    let col = rect(col.x, col.y, work.x + work.w - col.x, col.h);
                    (
                        part(work, 0, 0, 720, 1000),
                        rows(&vec![1; m], Align::Stretch, col, 0, 1),
                    )
                }
                Others::Corner => (
                    work,
                    (0..m as i32)
                        .map(|j| {
                            cell(
                                part(
                                    work,
                                    990 - (3 - j % 3) * 190,
                                    (980 - (1 + j / 3) * 220).max(0), // 13+: stack at the top
                                    180,
                                    200,
                                ),
                                work,
                            )
                        })
                        .collect(),
                ),
                Others::Minimize => (
                    work,
                    vec![
                        Target {
                            minimize: true,
                            ..full(0)
                        };
                        m
                    ],
                ),
                Others::Behind => (work, (0..m).map(|j| full(i64::MIN + j as i64)).collect()),
            };
            let mut rest = rest.into_iter();
            (0..n)
                .map(|i| {
                    if i == main {
                        Target {
                            cell: big,
                            ..full(i64::MAX)
                        }
                    } else {
                        rest.next().expect("one target per other window")
                    }
                })
                .collect()
        }
        Layout::Wide | Layout::Focus { .. } => (0..n).map(|_| full(0)).collect(),
    }
}

/// Running windows as (rank, pid, window) to slot order: entry i goes to the layout's slot i
/// (tile i in the UI). Rank = index of the window's account in the wanted order
/// (`usize::MAX`: unknown, goes last, by pid). Accounts that aren't running leave no hole:
/// later windows move up a slot. One entry per pid.
pub fn slots<T>(mut wins: Vec<(usize, u32, T)>) -> Vec<(usize, u32, T)> {
    wins.sort_by_key(|w| (w.0, w.1));
    wins.dedup_by_key(|w| w.1);
    wins
}

/// A suggested layout for the step-2 picker.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub layout: Layout,
}

/// The 3 or 4 best layouts for `count` windows, best first (1: just full screen). Focus
/// layouts carry `main: ""`: `grid_arrange` reads that as the first account of the order.
pub fn suggest(count: usize) -> Vec<Suggestion> {
    let s = |label: String, layout| Suggestion { label, layout };
    let grid = |c: &[u32]| {
        let label = match c {
            [2] => "Side by side".into(),
            [k] => format!("{k} across"),
            [k, rest @ ..] if rest.iter().all(|r| r == k) => format!("{k} x {}", c.len()),
            _ => c.iter().map(u32::to_string).collect::<Vec<_>>().join(" + "),
        };
        let layout = Layout::Rows {
            counts: c.to_vec(),
            align: Align::Center,
        };
        s(label, layout)
    };
    let focus = |others| {
        let main = String::new();
        s(
            format!("Main big + {} small", count - 1),
            Layout::Focus { main, others },
        )
    };
    let cascade = s("Cascade".into(), Layout::Cascade);
    let auto: Vec<u32> = auto_rows(count).into_iter().map(|k| k as u32).collect();
    match count {
        0 => vec![],
        1 => vec![s("Full screen".into(), Layout::Single)],
        2 => vec![grid(&[2]), focus(Others::Side), cascade],
        3 => vec![grid(&[3]), focus(Others::Side), grid(&[1, 2]), cascade],
        7 => vec![focus(Others::Strip), grid(&auto), cascade],
        10 => vec![grid(&[5, 5]), focus(Others::Strip), cascade],
        _ => vec![grid(&auto), focus(Others::Strip), cascade],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_by_count() {
        for n in 1..=10 {
            let s = suggest(n);
            assert!(
                n == 1 && s.len() == 1 || (3..=4).contains(&s.len()),
                "n={n}"
            );
            for x in &s {
                assert_eq!(plan(n, WORK, &x.layout, 0).len(), n, "n={n} {}", x.label);
            }
            let mut labels: Vec<_> = s.iter().map(|x| &x.label).collect();
            labels.sort();
            labels.dedup();
            assert_eq!(labels.len(), s.len(), "n={n}");
        }
        let labels = |n| suggest(n).into_iter().map(|x| x.label).collect::<Vec<_>>();
        assert_eq!(labels(1), ["Full screen"]);
        assert_eq!(labels(2), ["Side by side", "Main big + 1 small", "Cascade"]);
        assert_eq!(labels(4)[0], "2 x 2");
        assert_eq!(labels(5)[0], "3 + 2");
        assert_eq!(labels(6), ["3 x 2", "Main big + 5 small", "Cascade"]);
        assert_eq!(labels(7), ["Main big + 6 small", "4 + 3", "Cascade"]);
        assert_eq!(labels(9)[0], "3 x 3");
        assert_eq!(
            suggest(6)[1].layout,
            Layout::Focus {
                main: String::new(),
                others: Others::Strip
            }
        );
        assert!(suggest(0).is_empty());
    }

    /// Slot i on screen = entry i: wanted order first, missing accounts close up.
    #[test]
    fn corner_stays_on_screen() {
        let work = rect(0, 0, 1920, 1040);
        let l = Layout::Focus {
            main: String::new(),
            others: Others::Corner,
        };
        assert!(plan(20, work, &l, 0).iter().all(|t| t.cell.y >= work.y));
    }

    #[test]
    fn slot_mapping() {
        // Wanted order: A(0) B(1) C(2) D(3); B isn't running, pid 5 is unknown.
        let wins = vec![
            (3, 40, 'D'),
            (usize::MAX, 5, '?'),
            (0, 10, 'A'),
            (2, 30, 'C'),
            (0, 10, 'A'),
        ];
        let got: Vec<char> = slots(wins).into_iter().map(|w| w.2).collect();
        assert_eq!(got, ['A', 'C', 'D', '?']);
        // Slot 0 top-left, slot 1 right of it (3 x 2 rows).
        let t = plan(4, WORK, &suggest(6)[0].layout, 0);
        assert_eq!((t[0].cell.x, t[0].cell.y), (0, 0));
        assert!(t[1].cell.x > t[0].cell.x && t[1].cell.y == 0);
        assert!(t[3].cell.y > 0);
    }

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        w: 2560,
        h: 1400,
    };

    fn inside(t: &Target, a: Rect) -> bool {
        let c = t.cell;
        c.x >= a.x && c.y >= a.y && c.x + c.w <= a.x + a.w && c.y + c.h <= a.y + a.h
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    fn no_overlap(t: &[Target]) -> bool {
        (0..t.len()).all(|i| (i + 1..t.len()).all(|j| !overlaps(t[i].cell, t[j].cell)))
    }

    fn focus(others: Others) -> Layout {
        Layout::Focus {
            main: "a".into(),
            others,
        }
    }

    #[test]
    fn serde_shapes() {
        let l: Layout = serde_json::from_str(r#"{"type":"rows","counts":[3,2]}"#).unwrap();
        assert_eq!(
            l,
            Layout::Rows {
                counts: vec![3, 2],
                align: Align::Center
            }
        );
        let l: Layout =
            serde_json::from_str(r#"{"type":"focus","main":"x","others":"minimize","count":3}"#)
                .unwrap();
        assert_eq!(
            l,
            Layout::Focus {
                main: "x".into(),
                others: Others::Minimize
            }
        );
        assert_eq!(
            serde_json::to_value(Layout::default()).unwrap(),
            serde_json::json!({"type":"columns","count":3})
        );
    }

    #[test]
    fn auto_rows_by_count() {
        let want: [&[usize]; 10] = [
            &[1],
            &[2],
            &[3],
            &[2, 2],
            &[3, 2],
            &[3, 3],
            &[4, 3],
            &[4, 4],
            &[3, 3, 3],
            &[4, 4, 2],
        ];
        for (i, w) in want.iter().enumerate() {
            let n = i + 1;
            assert_eq!(auto_rows(n), *w, "n={n}");
            let t = plan(n, WORK, &Layout::Auto, 0);
            assert_eq!(t.len(), n);
            assert!(t.iter().all(|t| inside(t, WORK)), "n={n}");
            assert!(no_overlap(&t), "n={n}");
        }
        assert_eq!(auto_rows(17), vec![5, 5, 5, 2]);
    }

    #[test]
    fn rows_short_row_centered() {
        let t = plan(
            5,
            WORK,
            &Layout::Rows {
                counts: vec![3, 2],
                align: Align::Center,
            },
            0,
        );
        let w = 2560 / 3;
        assert_eq!(t[0].cell, rect(0, 0, w, 700));
        assert_eq!(t[3].cell, rect((2560 - 2 * w) / 2, 700, w, 700));
        assert_eq!(t[4].cell.x + w, 2560 - t[3].cell.x);
        // Leftovers repeat the last count; extra counts are dropped.
        assert_eq!(fit_counts(&[2, 3], 9), vec![2, 3, 3, 1]);
        assert_eq!(fit_counts(&[4, 3], 2), vec![2]);
        assert_eq!(fit_counts(&[], 5), vec![3, 2]);
    }

    /// Today's Grid Arrange: 3 wide, flush edges, middle centered, bottom row on the taskbar.
    #[test]
    fn min_size_overlaps_flush() {
        let work = rect(0, 0, 1920, 1032);
        let size = (802, 631);
        let t = plan(5, work, &Layout::default(), 0);
        assert_eq!(t[0].pos(work, size), (0, 0));
        assert_eq!(t[1].pos(work, size), (559, 0));
        assert_eq!(t[2].pos(work, size).0 + size.0, 1920);
        assert_eq!(t[4].pos(work, size), (559, 1032 - 631));
        // Big enough screen: no overlap, no gap.
        let big = rect(0, 0, 2400, 1400);
        assert_eq!(
            plan(2, big, &Layout::default(), 0)[1].pos(big, (800, 700)),
            (800, 0)
        );
        assert_eq!(
            plan(7, work, &Layout::default(), 0)[6].pos(work, size).1 + size.1,
            1032
        );
        // Middle over its neighbors, lower row over upper.
        assert!(t[1].z > t[0].z && t[1].z > t[2].z && t[3].z > t[0].z);
        // Larger than the work area: pinned top-left.
        assert_eq!(t[4].pos(rect(0, 0, 700, 500), size), (0, 0));
    }

    #[test]
    fn preset_classic() {
        for n in [5, 7] {
            let t = plan(n, WORK, &Layout::Columns { count: 3 }, 0);
            assert_eq!(t[3].cell.x, 0, "left aligned");
            assert_eq!(t[3].cell.y, t[0].cell.h);
            assert!(no_overlap(&t));
        }
    }

    #[test]
    fn preset_even() {
        for (n, last_row) in [(5, 2), (7, 3)] {
            let t = plan(n, WORK, &Layout::Auto, 0);
            let first = &t[n - last_row];
            assert_eq!(
                first.cell.x,
                WORK.w - t[n - 1].cell.x - t[n - 1].cell.w,
                "centered"
            );
            assert!(no_overlap(&t));
        }
    }

    #[test]
    fn preset_bands() {
        for n in [5, 7] {
            let t = plan(n, WORK, &Layout::Bands, 0);
            let top = n.div_ceil(2);
            assert_eq!(t[top].cell.x, 0);
            assert!(t[n - 1].cell.x + t[n - 1].cell.w > WORK.w - 3, "stretched");
            assert!(t.iter().all(|t| t.cell.h == 700));
        }
    }

    #[test]
    fn preset_pyramid() {
        for (n, rows_) in [(5, 3), (7, 4)] {
            let t = plan(n, WORK, &Layout::Pyramid, 0);
            assert_eq!(t[0].cell.h, WORK.h / rows_);
            assert!(
                (t[0].cell.x * 2 + t[0].cell.w - WORK.w).abs() <= 1,
                "apex centered"
            );
            assert!(no_overlap(&t));
        }
    }

    #[test]
    fn preset_wide() {
        for n in [5, 7] {
            let t = plan(n, WORK, &Layout::Wide, 0);
            assert_eq!(t[0].cell, part(WORK, 0, 0, 600, 1000));
            assert!(t[1..].iter().all(|x| x.cell.x >= 1536));
            assert!(no_overlap(&t));
        }
    }

    #[test]
    fn preset_cascade() {
        for n in [5, 7] {
            let t = plan(n, WORK, &Layout::Cascade, 0);
            assert_eq!(t[0].cell.x, 0);
            let r = t[n - 1].cell.x + t[n - 1].cell.w;
            assert!(r <= WORK.w && r > WORK.w - n as i32, "last flush right");
            assert!(t
                .windows(2)
                .all(|p| p[1].cell.y > p[0].cell.y && p[1].z > p[0].z));
        }
    }

    /// Everything but the big / full-screen window asks for Roblox's minimum size.
    #[test]
    fn only_big_windows_keep_their_cell_size() {
        let small =
            |l: &Layout| -> Vec<bool> { plan(5, WORK, l, 0).iter().map(|t| t.small).collect() };
        let rest = [false, true, true, true, true];
        assert_eq!(small(&Layout::Cascade), [true; 5]);
        assert_eq!(small(&Layout::default()), [true; 5]);
        assert_eq!(small(&Layout::Single), [false; 5]);
        assert_eq!(small(&Layout::Wide), rest);
        assert_eq!(small(&focus(Others::Strip)), rest);
        assert_eq!(small(&focus(Others::Side)), rest);
        let t = plan(5, WORK, &Layout::Cascade, 0);
        assert_eq!(t[0].size(), (1, 1));
        // Minimum-size windows still run corner to corner.
        assert_eq!(t[0].pos(WORK, (800, 600)), (0, 0));
        assert_eq!(t[4].pos(WORK, (800, 600)), (WORK.w - 800, WORK.h - 600));
        assert_eq!(
            plan(5, WORK, &Layout::Single, 0)[0].size(),
            (WORK.w, WORK.h)
        );
    }

    #[test]
    fn preset_focus_strip() {
        for n in [5, 7] {
            let t = plan(n, WORK, &focus(Others::Strip), 2);
            assert_eq!(t[2].z, i64::MAX);
            assert_eq!(t[2].cell, part(WORK, 80, 0, 840, 720));
            assert!(t
                .iter()
                .enumerate()
                .all(|(i, x)| i == 2 || x.cell.y == 1036));
            assert!(no_overlap(&t));
            // Strip too short for 800x600: flush with the bottom, over the big window's edge.
            let (_, y) = t[0].pos(WORK, (800, 600));
            assert_eq!(y, WORK.h - 600);
        }
    }

    #[test]
    fn preset_focus_side() {
        for n in [5, 7] {
            let t = plan(n, WORK, &focus(Others::Side), 0);
            assert_eq!(t[0].cell, part(WORK, 0, 0, 720, 1000));
            assert!(t[1..].iter().all(|x| x.cell.x == 1843));
            assert!(no_overlap(&t));
            // Spread flush top to bottom when windows are taller than their share.
            assert_eq!(t[1].pos(WORK, (800, 600)), (WORK.w - 800, 0));
            assert_eq!(t[n - 1].pos(WORK, (800, 600)).1, WORK.h - 600);
        }
    }

    #[test]
    fn preset_focus_minimize() {
        for n in [5, 7] {
            let t = plan(n, WORK, &focus(Others::Minimize), 1);
            assert!(!t[1].minimize && t[1].cell == WORK);
            assert_eq!(t.iter().filter(|x| x.minimize).count(), n - 1);
        }
    }

    #[test]
    fn preset_focus_behind() {
        for n in [5, 7] {
            let t = plan(n, WORK, &focus(Others::Behind), 0);
            assert!(t.iter().all(|x| x.cell == WORK && !x.minimize));
            assert!(t[1..].iter().all(|x| x.z < t[0].z));
        }
    }

    #[test]
    fn preset_focus_corner() {
        for n in [5, 7] {
            let t = plan(n, WORK, &focus(Others::Corner), 0);
            assert_eq!(t[0].cell, WORK);
            assert!(t[1..]
                .iter()
                .all(|x| inside(x, WORK) && x.cell.x > WORK.w / 3 && x.cell.w < WORK.w / 4));
            assert!(no_overlap(&t[1..]));
        }
    }

    #[test]
    fn single_and_tiny() {
        let t = plan(3, WORK, &Layout::Single, 0);
        assert!(t.iter().all(|x| x.cell == WORK) && t[0].z > t[1].z);
        assert!(plan(0, WORK, &Layout::Auto, 0).is_empty());
        assert_eq!(plan(1, WORK, &focus(Others::Strip), 5)[0].cell, WORK);
        assert_eq!(plan(1, WORK, &Layout::Wide, 0)[0].cell, WORK);
    }

    #[test]
    fn every_layout_places_each_window() {
        let layouts = [
            Layout::Auto,
            Layout::Columns { count: 3 },
            Layout::Rows {
                counts: vec![3, 2],
                align: Align::Stretch,
            },
            Layout::Bands,
            Layout::Pyramid,
            Layout::Wide,
            Layout::Cascade,
            Layout::Single,
            focus(Others::Strip),
            focus(Others::Side),
            focus(Others::Corner),
            focus(Others::Minimize),
            focus(Others::Behind),
        ];
        for l in &layouts {
            for n in 0..=2 {
                assert_eq!(plan(n, WORK, l, 0).len(), n, "n={n} {l:?}");
            }
        }
    }

    #[test]
    fn stretch_rows_fill_width() {
        let t = plan(
            5,
            WORK,
            &Layout::Rows {
                counts: vec![3, 2],
                align: Align::Stretch,
            },
            0,
        );
        assert_eq!(t[0].cell.w, 2560 / 3);
        assert_eq!(t[3].cell.w, 2560 / 2);
        assert_eq!(t[4].cell.x + t[4].cell.w, 2560);
        assert!(no_overlap(&t));
    }

    #[test]
    fn fit_counts_zeros_and_columns_floor() {
        assert_eq!(fit_counts(&[0, 0], 5), vec![3, 2]);
        assert_eq!(fit_counts(&[0, 4], 4), vec![4]);
        assert_eq!(
            plan(3, WORK, &Layout::Columns { count: 0 }, 0),
            plan(3, WORK, &Layout::Columns { count: 1 }, 0)
        );
        // A huge count (hand-edited settings) is capped, never wrapped to a negative width.
        assert_eq!(
            plan(3, WORK, &Layout::Columns { count: u32::MAX }, 0),
            plan(3, WORK, &Layout::Columns { count: 100 }, 0)
        );
        assert!(plan(3, WORK, &Layout::Columns { count: u32::MAX }, 0)
            .iter()
            .all(|t| t.cell.w > 0));
    }

    #[test]
    fn bands_and_wide_small_counts() {
        assert!(plan(0, WORK, &Layout::Bands, 0).is_empty());
        assert_eq!(plan(1, WORK, &Layout::Bands, 0)[0].cell, WORK);
        let t = plan(3, WORK, &Layout::Bands, 0);
        assert_eq!((t[0].cell.w, t[2].cell.w), (WORK.w / 2, WORK.w));
        assert_eq!(t[2].cell.y, 700);
        // Wide side column: one per row up to 3 windows, then two per row.
        let t = plan(4, WORK, &Layout::Wide, 0);
        assert!(t[1].cell.y < t[2].cell.y && t[2].cell.y < t[3].cell.y);
        let t = plan(5, WORK, &Layout::Wide, 0);
        assert_eq!(t[1].cell.y, t[2].cell.y);
        assert!(t[3].cell.y > t[1].cell.y);
    }

    #[test]
    fn focus_main_index_clamped() {
        let t = plan(3, WORK, &focus(Others::Strip), 99);
        assert_eq!(t[2].z, i64::MAX);
        assert_eq!(t[2].cell, part(WORK, 80, 0, 840, 720));
    }

    #[test]
    fn suggest_labels_more_counts() {
        let labels = |n| suggest(n).into_iter().map(|x| x.label).collect::<Vec<_>>();
        assert_eq!(
            labels(3),
            ["3 across", "Main big + 2 small", "1 + 2", "Cascade"]
        );
        assert_eq!(labels(8)[0], "4 x 2");
        assert_eq!(labels(10)[0], "5 x 2");
    }

    #[test]
    fn wire_names_for_every_layout() {
        let rws = |counts: Vec<u32>, align: Align| Layout::Rows { counts, align };
        let cases = [
            (r#"{"type":"auto"}"#, Layout::Auto),
            (r#"{"type":"bands"}"#, Layout::Bands),
            (r#"{"type":"pyramid"}"#, Layout::Pyramid),
            (r#"{"type":"wide"}"#, Layout::Wide),
            (r#"{"type":"cascade"}"#, Layout::Cascade),
            (r#"{"type":"single"}"#, Layout::Single),
            (
                r#"{"type":"columns","count":4}"#,
                Layout::Columns { count: 4 },
            ),
            (
                r#"{"type":"rows","counts":[2,1],"align":"left"}"#,
                rws(vec![2, 1], Align::Left),
            ),
            (
                r#"{"type":"rows","counts":[2,1],"align":"stretch"}"#,
                rws(vec![2, 1], Align::Stretch),
            ),
            (
                r#"{"type":"focus","main":"a","others":"strip"}"#,
                focus(Others::Strip),
            ),
            (
                r#"{"type":"focus","main":"a","others":"side"}"#,
                focus(Others::Side),
            ),
            (
                r#"{"type":"focus","main":"a","others":"corner"}"#,
                focus(Others::Corner),
            ),
            (
                r#"{"type":"focus","main":"a","others":"behind"}"#,
                focus(Others::Behind),
            ),
        ];
        for (json, want) in cases {
            assert_eq!(
                serde_json::from_str::<Layout>(json).unwrap(),
                want,
                "{json}"
            );
        }
    }

    #[test]
    fn suggest_large_counts() {
        for n in [11, 13, 20, 50] {
            let s = suggest(n);
            assert!((3..=4).contains(&s.len()), "n={n}");
            for x in &s {
                assert_eq!(plan(n, WORK, &x.layout, 0).len(), n, "n={n} {}", x.label);
            }
        }
    }

    #[test]
    fn offset_work_area_keeps_cells_inside() {
        // Monitor left of and below the origin: no layout may assume (0, 0).
        let work = rect(-1920, 40, 1920, 1000);
        let layouts = [
            Layout::Auto,
            Layout::Columns { count: 3 },
            Layout::Bands,
            Layout::Pyramid,
            Layout::Wide,
            Layout::Cascade,
            focus(Others::Strip),
            focus(Others::Side),
            focus(Others::Corner),
        ];
        for l in &layouts {
            for n in [5, 7] {
                let t = plan(n, work, l, 0);
                assert!(t.iter().all(|x| inside(x, work)), "n={n} {l:?}");
            }
        }
    }
}
