# Remote control protocol (v1)

Shared by HomeLab and Roblox Multi Instance (RMI). Both apps serve the same viewer file
(`remote-viewer.js`) and speak this protocol, so a session looks and behaves the same in both.
Change this file first, then both apps.

## Transport

One WebSocket per session, viewer (browser) to host (the device being controlled, possibly
through a relay such as the HomeLab hub). Private network only (Tailscale). Never Funnel,
never a public listener. `permessage-deflate` off.

- Host to viewer: **binary messages only**. First byte is the type.
- Viewer to host: **text messages only**, one JSON object each, at most 8 KB (terminal
  input at most 64 KB). Unknown `t` is ignored.

Auth happens before or at the start of the socket and is app-specific (HomeLab: owner
session on the hub; RMI: pairing token in the first message, `{"t":"auth","token":"..."}`).
Nothing below is processed before auth succeeds. The token is never put in a URL.

## Host to viewer

| Type | Payload | Meaning |
|---|---|---|
| `0x01` | `u8 screen`, then a JPEG | Full frame of that screen, cursor drawn in. |
| `0x02` | UTF-8 JSON | Control message, see below. |
| `0x03` | raw bytes | Terminal output. |
| `0x06` | `u8 screen`, `u8 flags`, then H.264 | Only with cap `h264` after the viewer offered it, see below. |

Control messages (`0x02`):

- `{"t":"hello","proto":1,"host":"name","os":"windows|macos|linux","screens":[{"id":0,"w":1920,"h":1080,"primary":true}],"state":"...","canInput":true,"terminal":false}`
  sent once after auth.
- `{"t":"state","state":"...","canInput":true}` whenever it changes.
- `{"t":"pong","n":1}` reply to `ping`.
- `{"t":"bye","why":"..."}` before the host closes (stopped at the device, switched off).

`state` values and the words the viewer shows:

| State | Viewer text |
|---|---|
| `ready` | (none) |
| `needs_permission` | This device needs permission before it can share its screen. Open the app on it to finish. |
| `locked` | Locked, sign-in or admin prompt on screen. It cannot be viewed or controlled right now. |
| `no_display` | No screen on this device. Terminal only. |
| `wayland` | This Linux desktop uses Wayland, which is not supported yet. Terminal only. |
| `desktop_unsupported` | Screen sharing is not supported on this device yet. Terminal only. |
| `off` | Remote access is switched off on this device. |

## Viewer to host

| Message | Meaning |
|---|---|
| `{"t":"start","screen":0,"quality":"auto","maxFps":60}` | Start or retarget the stream; sent again mid session when the user changes quality or frame rate. `quality`: `auto`, `speed`, `quality`. `maxFps`: 1..60 (the viewer offers 15, 30, 60, default 60, remembered per browser); a host clamps it to what it supports (an older host may cap lower). |
| `{"t":"stop"}` | Pause the stream (tab hidden). |
| `{"t":"ack"}` | Viewer decoded a frame, or dropped it for a newer one. Host keeps at most 4 frames unacknowledged (`MAX_UNACKED`; older hosts 2) and sends the newest JPEG, dropping older ones (H.264: see below, never dropped once encoded). The viewer keeps one frame waiting while one decodes; a newer arrival replaces (and acks) the waiting one, and it draws on the next animation frame. |
| `{"t":"mm","x":0.5,"y":0.5}` | Pointer move. `x`,`y` are 0..1 of the streamed screen. |
| `{"t":"md","b":0,"x":0.5,"y":0.5}` / `{"t":"mu",...}` | Button down / up. `b`: 0 left, 1 middle, 2 right. |
| `{"t":"wh","dx":0,"dy":-120}` | Wheel, 120 per notch, positive `dy` scrolls down. |
| `{"t":"kd","code":"KeyA"}` / `{"t":"ku","code":"KeyA"}` | Key down / up, `KeyboardEvent.code` values. |
| `{"t":"text","s":"hello"}` | Typed text (phone keyboard, paste of at most 1000 characters). |
| `{"t":"combo","name":"cad"}` | Special combos: `lock`, and `cad` (Ctrl+Alt+Del). The viewer offers `cad` only when `hello` lists it in an optional `"combos":["cad"]` array; a normal user process cannot send it, so hosts today do not. |
| `{"t":"view","on":true}` | View only for this session: host ignores input, terminal `open` and `in` included, until `on` is false. |
| `{"t":"term","op":"open","cols":80,"rows":24}` | Terminal: `open`, `in` (`"d":"..."`), `resize`, `close`. Long input (a paste) goes in several `in` messages of at most 8000 characters. |
| `{"t":"ping","n":1}` | Latency probe. |

## Host rules

- Remote access is off until the owner switches it on **at that device**. Off means the
  socket is refused with state `off`.
- While a session is live the host shows an always-visible "being controlled" indicator
  with a Stop button, and a session is written to a log (who, when, how long, view or control).
- On disconnect the host releases every key and button it holds down. Stop at the device (or
  switching remote access off) ends input at once; the `bye` follows without waiting on it.
- One session at a time per device. After Stop at the device, new sessions are refused for
  30 s with `bye` "Stopped at the device."
- A relay drops a viewer that answers no WebSocket ping for 30 s (pings every 10 s), which ends
  the session as a disconnect.
- A shell that exits sends `\r\n[shell ended]\r\n` as terminal output; a new `open` starts another.
- A host in a dry run (testing) offers no terminal (`terminal:false`) and refuses `open`.
- Frames: JPEG quality about 70 (`auto`), about 50 (`speed`), about 85 (`quality`); skip a frame
  identical to the last one sent; scale down above 1920 px wide in `speed`.
- A host refusing a session (switched off, busy, offline) sends `bye` with the reason, after a
  `state` of `off` when it is switched off, then closes.
- No clipboard sync in v1 (typed paste of the viewer's own text is `text`).

## Additions (proto 1, optional, negotiated)

`hello` gains `"caps":[...]` (what this host supports) and `"screen":n` (the one streaming).
A viewer uses a feature only when its cap is listed. Old hosts send no `caps`.

| Cap | Meaning |
|---|---|
| `screens` | Host sends `{"t":"screens","screens":[...],"screen":n}` (type `0x02`) whenever the monitor list or the streamed screen changes. The viewer replaces its list. A host never ends a session only because monitors changed. |
| `all` | Screen id `255` is every monitor in one picture (the whole virtual desktop). `start.screen` 255 selects it, frames carry 255, and `x`,`y` are 0..1 of that picture. `screens` entries carry `"x","y"` (top-left in virtual desktop pixels). A host may always list the cap; the viewer shows the choice only with 2 or more screens. |
| `cursor` | `start` may carry `"cursor":true`: frames then leave the pointer out and the host sends `{"t":"cur","x":0.5,"y":0.5,"shape":"default","vis":true}` when it moves or changes (at most 30 per second). `shape` is a CSS cursor name (`default`, `text`, `pointer`, `move`, `ew-resize`, `ns-resize`, `nwse-resize`, `nesw-resize`, `wait`, `crosshair`, `not-allowed`); unknown is `default`. |
| `stats` | With `quality` `auto` the host adapts JPEG quality (about 40 to 80) and scale (1, 0.75, 0.5) to how fast frames are acknowledged, and sends `{"t":"stats","q":70,"scale":1,"fps":12,"kbps":900}` every 2 s. Optional extra fields (a viewer shows only the ones present): `cap` capture ms per frame, `enc` JPEG encode ms per frame, `kb` KB per frame, `inflight` frames sent and not yet acked, `w` streamed width in pixels. The viewer's Show stats overlay adds its own decode ms (`dec`), ack-to-next-frame ms (`ack`) and ping `rtt`. |
| `takeover` | See below. |
| `media` | Key codes `AudioVolumeUp`, `AudioVolumeDown`, `AudioVolumeMute`, `MediaPlayPause`, `MediaTrackNext`, `MediaTrackPrevious`, `MediaStop`, `Pause` are handled. |
| `h264` | The host can stream H.264 instead of JPEG. See below. |

`bye` gains `"code"` next to the human `"why"`: `off`, `busy`, `auth`, `removed`, `stopped`,
`timeout`, `network`, `replaced`, `offline`, `old`. The viewer shows its own sentence per
code and falls back to `why`. A `busy` bye also carries `"by":"<who holds the session>"`.

Take over: a refused viewer may connect again asking to take over (RMI: `"takeover":true`
in the `auth` message; HomeLab: `takeover=1` on the socket URL). The host ends the old
session with code `replaced` and serves the new one. A reconnect from the same paired
device or the same signed-in owner always replaces its own stale session without asking.

Paste: on Ctrl+V / Cmd+V the viewer may read its own clipboard text and send it as `text`
messages of at most 1000 characters each instead of the key press; hosts type every chunk.

Viewer expectations of a host: answer `ping` with `pong` promptly even when no frames flow
(a viewer treats 8 s of silence as a lost connection); after a `screens` message stream the
`screen` it names; type `text` with no modifier keys held; accept `text` messages up to 8 KB.

Key repeat: a viewer repeats `kd` for a held key; hosts accept a `kd` for a key already down.

## H.264 (optional cap `h264`)

Still proto 1: negotiated per stream, so either side can be older.

- The viewer offers it only when the host lists `h264` and its own decoder (WebCodecs
  `VideoDecoder`, `avc1.42E01F`, `optimizeForLatency`) says it is supported:
  `{"t":"start",...,"codecs":["h264"]}`. A host that gets no `codecs` (or no `"h264"` in it)
  streams JPEG `0x01` as before. Every `start` chooses again.
- Frames are `0x06`, `u8 screen`, `u8 flags` (bit 0: IDR keyframe), then one Annex B access
  unit: baseline or main profile, no B-frames, SPS and PPS in band before every IDR (the
  viewer configures its decoder without a `description`, from the SPS's profile and level).
  Width and height are even.
- The first frame of every `start` is an IDR. After that an IDR comes only when the size
  changes, after an encoder error, or on request:
  `{"t":"idr"}` from the viewer (after a decode error); the host then sends a keyframe even
  of a still screen. No periodic keyframes (the link is TCP, nothing is lost).
- Flow control is the same `ack` window, but nothing is dropped once encoded: a host skips or
  replaces pictures only before encoding them, and sends every encoded frame in order. The
  viewer decodes every chunk in order and acks each once the decoder has taken it (WebCodecs
  `dequeue`; at hand-over where that event is missing), so a decoder that holds pictures back
  never stalls the window; a chunk it drops (another screen, a delta while it waits for a
  keyframe) is acked at once.
  Drawing stays latest-wins: a decoded picture not drawn yet is replaced by the next one.
- Viewer decode error: a new decoder, `idr` (again every second while it waits), deltas acked
  and dropped until the next keyframe. A configuration the browser turns down after all, or 3
  errors without a picture in between: `start` again without `codecs` (JPEG for the rest of
  the session). A host honours `idr` at most every 250 ms.
- `stats` gains `"codec":"h264"|"jpeg"`; with H.264 it carries `br` (target kbit/s) instead
  of `q`, and `encoder`, a short name of the encoder in use (for example `NVENC via MF`,
  `software MF`, `VideoToolbox`). `enc` is then H.264 encode ms per frame.
- A host that lists `h264` but cannot start an encoder streams JPEG `0x01` instead; the viewer
  handles both frame types whatever it offered. RMI skips an encoder that fails before its
  first frame or twice in a row and tries the next; none left means JPEG.
- RMI's bitrates: `quality` 25 Mbit/s at full size; `speed` 6 Mbit/s at most 1280 px wide;
  `auto` starts at 16 Mbit/s and steps 25, 16, 10, 6 Mbit/s on the same signals as JPEG's
  quality, then 3 Mbit/s at 0.75 scale as the last step (at most 1920 px wide). Bitrate steps
  apply without a keyframe. Encoder limit 3840 x 2160 (larger pictures are scaled down, tiny
  ones up to at least 256 x 128). Encoders: Windows Media Foundation (the GPU's hardware MFT:
  NVIDIA, then AMD, then Intel; else Microsoft's software MFT), macOS VideoToolbox. When the
  screen stops changing RMI encodes the last picture 15 more times so rate control sharpens it.

## Files, sound, secure desktop (optional caps)

**`files`** (control sessions only, never in view only; every transfer is written to the
session log with name, size and direction).

- Browse: `{"t":"fs","op":"list","path":""}` (`""` is the signed-in user's home folder).
  Reply (type `0x02`): `{"t":"fs","path":"...","parent":"...","entries":[{"name":"a.txt","dir":false,"size":12,"mtime":1700000000}]}`
  (at most 2000 entries, folders first) or `{"t":"fs","path":"...","err":"..."}`.
- Download (host to viewer): `{"t":"file","op":"get","id":7,"path":"..."}`. Host replies
  `{"t":"file","id":7,"name":"a.txt","size":12}`, then binary `0x04`, `u32` big-endian id,
  then at most 256 KB of bytes per message, then `{"t":"file","id":7,"done":true}`. The viewer
  sends `{"t":"file","op":"ack","id":7,"got":<total bytes received>}`; the host keeps at most
  4 MB unacknowledged.
- Upload (viewer to host): `{"t":"file","op":"put","id":8,"name":"b.zip","size":34}`, then
  **binary messages from the viewer** (the only binary a viewer sends): `0x10`, `u32`
  big-endian id, at most 256 KB of bytes; then `{"t":"file","op":"end","id":8}`. The host
  sends `{"t":"file","id":8,"got":<total>}` at least every 1 MB (the viewer stays at most
  4 MB ahead) and finally `{"t":"file","id":8,"done":true,"path":"<where it was saved>"}`.
  Uploads land in the signed-in user's Downloads folder. The host keeps only the base name,
  strips path separators and reserved names, and never overwrites (`b (1).zip`).
- Either side cancels with `{"t":"file","op":"cancel","id":n}`; a failure is
  `{"t":"file","id":n,"err":"..."}`. Limit 4 GB per file, 2 transfers at a time. A host
  deletes a partial upload on cancel, error or disconnect.

**`audio`**: `{"t":"audio","on":true}` starts the device's sound output stream, `false`
stops it. Host replies `{"t":"audio","rate":48000,"ch":2}` then binary `0x05` messages of
raw signed 16-bit little-endian PCM, about 20 ms each. Sound is never queued: a host drops
audio when more than 200 ms is waiting. Off until the viewer asks.

**`secure`**: this host can show and control the lock screen, the sign-in screen and admin
(UAC) prompts. It then does not report `locked` for them and lists `cad` in `combos`. Only
a host that runs as a system service can offer it, and only after the owner switched it on
at the device separately from remote access itself.

## What cannot work

- Windows: UAC prompts, the lock screen and windows running as administrator cannot be seen
  or clicked from a normal user session (a host with cap `secure` can).
- macOS: needs Screen Recording and Accessibility permission, granted at the Mac. Not
  available at the login window.
- Linux: Wayland and X11 desktops and headless machines get the terminal only.

## Host notes (HomeLab)

How HomeLab fills in what the spec leaves open:

- The hub adds `takeover` to the agent's `hello.caps` (the hub does the take over). The owner is one identity, so
  any new HomeLab viewer replaces a live session; `busy` comes only while that session is still letting go (`by`:
  `this device's browser`, the Tailscale login name, or `another session` from the agent).
- `bye` codes: Settings, Features switched off sends `off` (its own `why`); the agent's side going silent or closing
  without a `bye` sends `network`; no answer from the device sends `timeout`. A `bye` with no fitting code (the banner
  could not be shown, the helper could not start) carries `why` only.
- `stats` is sent every 2 s while streaming in every quality mode; only `auto` adapts. Auto frames are capped at about
  1920 x 1200 pixels (area), so a 4K screen or the whole desktop is scaled down.
- `cur`: `vis` is false when the pointer is hidden or outside the streamed picture. Windows' arrow-with-hourglass is
  `wait`.
- macOS reports `desktop_unsupported` until screen sharing is built there.
- The hub drops a viewer silent for 10 s, not 30 s (the viewer's own `ping` every 2 s counts), so a reconnect is not
  held up by its dead predecessor; it pings the agent's side too and ends the session after 30 s of silence there.
- Windows hosts also send `"combos":["lock"]` in `hello`.
- `hello` may carry `"note":"..."`, a sentence the viewer shows beside the state. Today: a `secure` host whose
  Ctrl+Alt+Del is blocked by the device's `SoftwareSASGeneration` policy says so there (and still lists `cad`).
- Files: a transfer `id` is 1 to 4294967295; a message with another id is ignored. `fs` and `file` in view only get an
  `err` ("View only: ..."), and switching view only on ends running transfers with an `err`. `fs` replies echo the
  requested `path` on an error; `parent` is left out at a root (`C:\`, `/`). Other Windows drives are reached by
  listing their root path. A `get` of a folder or of more than 4 GB is an `err`. An upload sends a final `got` (the
  whole size) right before `done`; `done.path` is where it was saved (its name may differ from `put.name`). Uploads
  are written to `<name>.part`, created with "fail if anything of that name exists" (a link or junction counts), and
  on `end` renamed to the first free final name the same way (never over a file or through a link); the `.part` is
  deleted on cancel, error or disconnect.
- The viewer gives an `fs` request 10 s ("The device did not answer. Try again."); a later answer still shows. A
  download that gets more bytes than its announced `size` ends with an `err` and a `cancel`.
- Sound: `{"t":"audio","rate":48000,"ch":2}` is sent again when the default output device changes to another format;
  `ch` is at most 2 (surround sends front left and right). A host that cannot start it answers
  `{"t":"audio","on":false,"err":"..."}` (the viewer switches its sound button off and shows the `err` for a few
  seconds); after a capture error it waits at least 200 ms before trying again. View only keeps sound. Windows only.
- `secure`: switched on at the device with `homelab remote secure on` (agent.json `RemoteSecure`, next to `Remote`;
  only meaningful with `Remote` on; the hub cannot set it). The screen and input helper then runs as SYSTEM in the
  console session and follows the input desktop; files, terminal and sound run in a second helper as the signed-in
  user, started on first need. With nobody signed in, `fs` and `file` get an `err`, a terminal `open` gets a
  "Could not start a shell" line and `audio` an `err`. A transfer under way when that user's session ends gets an `err`,
  and an `fs`, `get`, `put`, terminal `open` or sound start sent while that helper is going away is answered the same
  way. Only `0x03`, `0x04`, `0x05` and `fs`, `file` and `audio` control messages pass from that helper; anything
  else it sends is dropped. When the console moves to another session (fast user switching, a remote desktop
  sign-in), the SYSTEM helper sends `bye` `network` ("The signed-in session on the device changed. Connect again.")
  and ends, and the user helper with it. The being-controlled banner shows on the Default desktop only, never on the
  secure desktop.
- `hello.note` shows in the connection chip's tooltip and once (per viewer, not per reconnect) as a dismissable notice.
- Relay sizes: the hub passes viewer binary only when it is `0x10` and at most 256 KB + 5 bytes; viewer text over
  64 KB still ends the session. Host `0x04` over 256 KB + 5 bytes and `0x05` over 64 KB are dropped. A finished
  transfer is one row in the device's Recent list (direction, who, name cut to 200 bytes, size); the hub follows at
  most 16 transfers per session for that, more still relay but are not listed. Any `fs` or `file` message makes the
  session "with control".
