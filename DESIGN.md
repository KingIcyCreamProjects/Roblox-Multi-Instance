---
name: Roblox Multi Instance
description: Calm, near-black account launcher where avatars and game art supply the colour and one chartreuse "go" accent marks the action.
colors:
  ink-1000: "oklch(0.145 0.006 255)"
  ink-950: "oklch(0.17 0.007 255)"
  ink-900: "oklch(0.2 0.008 255)"
  ink-850: "oklch(0.235 0.009 255)"
  ink-800: "oklch(0.275 0.01 255)"
  paper-0: "oklch(1 0 0)"
  paper-50: "oklch(0.977 0.003 255)"
  paper-100: "oklch(0.955 0.004 255)"
  paper-150: "oklch(0.925 0.005 255)"
  paper-200: "oklch(0.89 0.006 255)"
  fog-100: "oklch(0.94 0.004 255)"
  fog-300: "oklch(0.74 0.008 255)"
  fog-500: "oklch(0.66 0.01 255)"
  slate-900: "oklch(0.2 0.01 255)"
  slate-700: "oklch(0.42 0.012 255)"
  slate-500: "oklch(0.48 0.012 255)"
  green-light: "oklch(0.86 0.18 125)"
  green-light-hover: "oklch(0.9 0.17 124)"
  green-light-deep: "oklch(0.8 0.18 126)"
  green-light-olive: "oklch(0.46 0.13 130)"
  green-light-ink: "oklch(0.22 0.04 125)"
  status-online: "oklch(0.76 0.16 156)"
  status-ingame: "oklch(0.72 0.14 248)"
  status-studio: "oklch(0.76 0.15 58)"
  status-warn: "oklch(0.83 0.14 82)"
  status-error: "oklch(0.69 0.18 24)"
  on-image: "oklch(0.97 0.004 255)"
typography:
  display:
    fontFamily: "Geist Variable, Segoe UI Variable, system-ui, sans-serif"
    fontSize: "2.75rem"
    fontWeight: 600
    lineHeight: 1.05
    letterSpacing: "-0.03em"
  display-hero:
    fontFamily: "Geist Variable, Segoe UI Variable, system-ui, sans-serif"
    fontSize: "4rem"
    fontWeight: 600
    lineHeight: 1.02
    letterSpacing: "-0.035em"
  headline:
    fontFamily: "Geist Variable, Segoe UI Variable, system-ui, sans-serif"
    fontSize: "1.75rem"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.02em"
  title:
    fontFamily: "Geist Variable, Segoe UI Variable, system-ui, sans-serif"
    fontSize: "1.25rem"
    fontWeight: 600
    letterSpacing: "-0.01em"
  body:
    fontFamily: "Geist Variable, Segoe UI Variable, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: 1.45
  label:
    fontFamily: "Geist Variable, Segoe UI Variable, system-ui, sans-serif"
    fontSize: "0.8125rem"
    fontWeight: 500
  data:
    fontFamily: "Geist Mono Variable, ui-monospace, Cascadia Mono, monospace"
    fontSize: "0.75rem"
    fontWeight: 400
rounded:
  chip: "6px"
  control: "8px"
  panel: "10px"
  pill: "999px"
spacing:
  titlebar: "40px"
  sidebar: "220px"
  sidebar-rail: "64px"
  inspector: "360px"
  playbar: "64px"
  row: "52px"
  row-compact: "40px"
  control: "32px"
components:
  button-primary:
    backgroundColor: "{colors.green-light}"
    textColor: "{colors.green-light-ink}"
    rounded: "{rounded.control}"
    height: "32px"
    padding: "0 14px"
  button-primary-hover:
    backgroundColor: "{colors.green-light-hover}"
  button-secondary:
    backgroundColor: "{colors.ink-850}"
    textColor: "{colors.fog-100}"
    rounded: "{rounded.control}"
    height: "32px"
    padding: "0 14px"
  button-play:
    backgroundColor: "{colors.green-light}"
    textColor: "{colors.green-light-ink}"
    rounded: "{rounded.pill}"
    height: "40px"
    padding: "0 20px"
  input:
    backgroundColor: "{colors.ink-850}"
    textColor: "{colors.fog-100}"
    rounded: "{rounded.control}"
    height: "32px"
    padding: "0 10px"
  account-row:
    textColor: "{colors.fog-100}"
    rounded: "{rounded.control}"
    height: "{spacing.row}"
  chip-session:
    textColor: "{colors.status-warn}"
    rounded: "{rounded.pill}"
    padding: "2px 8px"
---

# Design System: Roblox Multi Instance

## Overview

**Creative North Star: "The Timing Tower"**

RMI sits beside bright Roblox windows in a dim room and is read in glances between rounds. The account list behaves like a motorsport timing tower: one tight column of drivers, each with a team-colour tick (the account colour tag), a live status mark and the current stint (game) in tabular rhythm. Everything else is quiet chrome on a cool near-black ground, so the game windows stay the brightest thing on screen.

Colour is borrowed, not painted. Avatar renders and game thumbnails carry the energy; the interface itself uses four lightness steps, hairlines, three text greys and a single chartreuse accent that means "go". The only blur and bloom in the system sit behind the inspector avatar and the Home hero, tinted from that image. Dense where work happens (accounts, the server table), spacious where decisions happen (inspector, game hero). Light theme is a secondary, equally tuned paper version of the same structure.

Source of truth: `src/styles/tokens.css` (primitive → semantic per theme → component) exposed to Tailwind v4 through `@theme inline` in `src/styles/index.css`. Components read semantic tokens only.

**Key Characteristics:**
- Near-black cool neutral ground with depth by lightness, not borders.
- One accent, chartreuse, only as a solid fill with dark ink or as the selection/focus ring.
- Status colours on small marks only: dots, pills.
- Rows are lists, not cards; cards only for objects (game tiles, outfits).
- Geist for everything; Geist Mono only for ids, JobIds, ports and logs.
- Quiet 120-180 ms motion, one authored moment: the Play capsule.

## Colors

A restrained cool-neutral palette (hue 255) with one saturated green-yellow accent and five reserved status hues.

### Primary
- **Green Light** (`oklch(0.86 0.18 125)`, light theme fill `oklch(0.8 0.18 126)`): the Play button, primary buttons, checked switches and checkboxes, selected group checkbox, the slider fill. Always a solid fill carrying **Green Light Ink** (`oklch(0.22 0.04 125)`) text.
- **Olive Ring** (`oklch(0.46 0.13 130)`, light theme only): the accent when it has to be read as text or as the focus ring on paper. Dark theme uses Green Light itself for `--accent-text`.

### Status (small marks only)
- **Online Emerald** (`oklch(0.76 0.16 156)`), **In-game Azure** (`oklch(0.72 0.14 248)`), **Studio Tangerine** (`oklch(0.76 0.15 58)`), **Session Amber** (`oklch(0.83 0.14 82)`), **Error Red** (`oklch(0.69 0.18 24)`). Light theme swaps each for its deeper `-deep` primitive so the marks pass on paper.
- **Account tags** (`--tag-1` … `--tag-8`): a fixed palette of eight, shown only as the 3 px left tick on a row and as swatches.

### Neutral
- **Ground** `--bg` (dark `ink-1000`, light `paper-50`): content area.
- **Surface 1** (`ink-950` / `paper-100`): title bar, sidebar, Play bar, segmented-control tracks.
- **Surface 2** (`ink-900` / `paper-0`): inspector, sheets, palette.
- **Surface 3** (`ink-850` / `paper-150`): inputs, menus, hovered rows, active tab.
- **Surface 4** (`ink-800` / `paper-200`): pressed, selected segment, avatar fallback, switch track.
- **Hairline** (white 7% / slate 9%) and **Hairline Strong** (12% / 15%): separators and input outlines.
- **Text 1 / 2 / 3** (dark `fog-100 / fog-300 / fog-500`, light `slate-900 / slate-700 / slate-500`): primary, secondary, tertiary.
- **On Image** (`oklch(0.97 0.004 255)`): text and switch/slider thumbs laid over imagery or accent; identical in both themes.

### Named Rules
**The Green Light Rule.** The accent means go or selected. It appears as a fill with dark ink, a 2 px ring, or a slider fill; never as glowing text, a gradient, or decoration.

**The Neutral When Blocked Rule.** A disabled or blocked primary (including the Play pill) drops the accent entirely to Surface 3 with Text 3. A 40% accent reads as muddy olive.

**The Small Marks Rule.** Status colours never fill an area larger than a pill. Selected rows use the accent at 10% (`bg-accent/10`), not a status hue.

## Typography

**UI Font:** Geist Variable (fallback Segoe UI Variable, system-ui)
**Data Font:** Geist Mono Variable (fallback ui-monospace, Cascadia Mono)

**Character:** a neutral workhorse sans with tight tracking on hero moments; mono is data, not costume.

### Hierarchy
- **Display Hero** (600, 64px, 1.02, -0.035em): Home hero game name at wide container widths (`@3xl`).
- **Display** (600, 44px, 1.05, -0.03em, balanced): vault screens, empty states, Home hero at narrow widths.
- **Headline** (600, 28px, leading-tight, -0.02em): inspector name, "3 accounts selected", Games hero tile.
- **Title** (600, 20px, -0.01em): sheet titles, section headings (Jump back in, Favorites). 16px semibold for screen titles in the 56px page header (Accounts, Discover, Settings sections).
- **Body** (400, 14px, 1.45): rows, descriptions. 13px for secondary lines and controls.
- **Label** (500, 13px): buttons, field labels; 12px for group headers, hints and tooltips.
- **Data** (Geist Mono, 12-13px): JobIds, place ids, ports, console log lines, file paths.

### Named Rules
**The Tabular Rule.** Every count, ping, player number, timestamp and badge number uses `tnum` (tabular numerals).

**The Mono Is Data Rule.** Mono only where the content is an identifier or machine output. Placeholders inside those fields switch back to the sans.

## Layout

A fixed desktop frame: 40px title bar, 220px sidebar (64px rail when collapsed), content, optional 360px resizable inspector (320-420), and a 64px Play bar pinned along the bottom on every screen after unlock. Supported windows 960×620 to 2560×1440 at 100-200% scaling; `--ui-scale` rescales the root font size from Settings.

Screen headers are 56px tall with 20-24px side padding. Spacing follows an 8-point grid with 4px half-steps inside controls; more space above a heading than below it. Lists are virtualized and use the `--row-h` token (52px comfortable, 40px compact).

Narrow behaviour is container-query driven, not breakpoint driven: the Accounts header drops the Add label to its icon and hides the search shortcut below a 34rem list column; the Play bar hides destination names and tightens the mode control below 52rem; the Home hero grows to 64px type above `@3xl`. Vault screens tighten their vertical gaps below 700px of window height.

## Elevation & Depth

Depth is tonal: four surface steps that differ in lightness, plus hairlines. One float shadow per theme, for floating layers only (menus, sheets, toasts, palette, tooltips, the dragged row), plus the thumb shadow for switch and slider thumbs. Focus rings are drawn with box-shadow but are not elevation.

### Shadow Vocabulary
- **Float, dark** (`0 18px 40px -12px oklch(0 0 0 / 0.6), 0 4px 10px -2px oklch(0 0 0 / 0.35)`).
- **Float, light** (`0 18px 40px -14px oklch(0.2 0.02 255 / 0.25), 0 3px 8px -2px oklch(0.2 0.02 255 / 0.12)`).
- **Thumb** (`0 1px 2-3px oklch(0 0 0 / 0.3-0.35)`): switch and slider thumbs only.

### Named Rules
**The One Bloom Rule.** Blur and colour bloom appear only behind the inspector avatar and the Home hero, tinted from that image (`useBloom`). Nowhere else; no glass.

## Shapes

One radius language: 6px chips (`--r-chip`), 8px controls (`--r-control`), 10px panels and tiles (`--r-panel`), pills only for the Play button, status/session pills and the hero Play. Checkboxes use 4px so they never read as radios. Avatars are rounded squares (control radius in rows, panel radius in stacks). The only coloured side bar is the 3px account tag tick, rounded on its right end.

## Components

### Buttons
- **Shape:** 8px radius, 32px tall (`--control-h`, 28px compact), 14px side padding, 13px medium label that never wraps.
- **Primary:** Green Light fill, Green Light Ink text, semibold; hover lifts to the lighter step, press to the deeper step.
- **Secondary:** Surface 3 fill, Text 1; hover/press Surface 4. **Ghost:** text only, Surface 3 on hover. **Danger:** Surface 3 with Error Red text.
- **Disabled:** primary goes neutral (see The Neutral When Blocked Rule); others drop to 40% opacity.
- **Icon button:** 32px square, Text 2, tooltip doubles as its accessible name.

### Segmented controls and tabs
Track on Surface 1 (or Surface 3 inside the Play bar), 2-4px inset; the selected segment sits one step up (Surface 3, or Surface 4 inside the Play bar) with Text 1. Used for Discover tabs, Add sheet tabs, Public/Private, grid/list and launch mode.

### Inputs / Fields
- **Style:** Surface 3 fill, transparent 1px border, 8px radius, 13px text, Text 3 placeholder; hover Surface 4, focus shows Hairline Strong.
- **Focus:** the global ring, `0 0 0 2px var(--bg), 0 0 0 4px var(--accent-text)` (2px accent ring at 2px offset).
- **Password fields:** Surface 2 with a Hairline Strong border, Error Red border plus alert text when rejected, and a 280ms shake (dropped under reduced motion).
- **Switch:** 36×20 pill, Surface 4 track, light thumb with a soft shadow; checked track is Green Light with an ink thumb.
- **Slider:** 4px neutral track with a Green Light fill up to the value, 16px light thumb that scales 1.12 on hover.

### Account row (signature)
52px row, 8px radius, 3px tag tick on the left, 32px avatar, alias over `@username · game`, session pill only when not ok, a presence dot in a fixed 20px column, and drag handle plus "…" revealed on hover. Selected rows fill with accent at 10%; the keyboard cursor row gets an inset 2px accent ring. Group headers are 36px sector labels with a count and a select-group checkbox.

### Play bar and capsule (signature)
Destination combobox (with a resolved chip: icon, name, players), optional server field, launch-mode segment, and the Play pill. Pressing Play morphs the pill into a progress capsule (layout animation, ~350ms expo-out): selected avatars fly from their rows into a stack, each ticking as its client launches, with "n of N" and Cancel; it settles back to Play. Reduced motion shows plain progress.

### Sheets and palette
Right-side sheets (Radix Dialog) on Surface 2 below the title bar with a scrim, 20px semibold title, 24px padding; widths 440-800px by content. The Ctrl/Cmd+K palette is a 560px Surface 2 panel 48px under the title bar with a 48px input and 36px options; active option on Surface 3 with an Enter glyph.

### Empty, loading and error states
Empty states are one centred composition: the mark, a 44px heading, one sentence, two actions. Loading uses skeleton rows shaped like the real layout (`--skeleton`, pulse). Errors are a line of text plus Retry, inline where the data would be.

## Do's and Don'ts

### Do:
- **Do** read only semantic tokens (`bg-surface-3`, `text-text-2`, `bg-accent`) in components; add new values to `tokens.css` in both themes.
- **Do** keep motion at 120-180ms with `--ease-out` (`cubic-bezier(0.16, 1, 0.3, 1)`) for menus, sheets, palette and inspector cross-fades; reserve 350ms for the Play morph and the post-unlock fade.
- **Do** honour reduced motion: the global CSS clamps durations and `MotionConfig` follows the `ui.reducedMotion` setting.
- **Do** mark every name and avatar with `data-pii` so privacy mode blurs it.
- **Do** let narrow widths collapse labels to icons (keeping the accessible name) before letting content overflow.

### Don't:
- **Don't** use eyebrow labels, gradient text, decorative glass, section numbers or hero-metric tiles.
- **Don't** add coloured side borders over 1px except the 3px account tag tick.
- **Don't** use emoji or Unicode glyphs as icons; Lucide at 1.75 stroke, 14-16px.
- **Don't** wrap rows or settings in cards, and never nest cards.
- **Don't** open a modal for work that does not need protected focus; use a sheet or inline editing.
- **Don't** use pure `#000` or pure white grounds.
