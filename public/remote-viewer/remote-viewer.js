/* remote-viewer.js: shared remote desktop viewer for HomeLab and Roblox Multi Instance (RMI).
   Wire protocol: HomeLab docs/remote-protocol.md (v1 plus Additions). Styles: remote-viewer.css (only --rv-* custom properties).
   Plain script, no dependencies:
     const s = RemoteViewer.open({ mount, url, auth, title, onClose });  // url is ws:// or wss://, never holds the token
     s.close();
   onClose(code) runs when the user presses Disconnect or Close. code is the bye code ('auth', 'removed', 'busy', ...)
   when the device ended or refused the session with one, otherwise undefined.
   Optional caps: files (Files panel, transfers), audio (Sound toggle), secure (note in the connection chip tooltip),
   h264 (H.264 frames through WebCodecs when this browser decodes them; JPEG otherwise).
   Remote data only ever reaches the DOM through textContent. */
(() => {
  'use strict';

  const TEXT = {
    needs_permission: 'This device needs permission before it can share its screen. Open the app on it to finish.',
    locked: 'Locked, sign-in or admin prompt on screen. It cannot be viewed or controlled right now.',
    no_display: 'No screen on this device. Terminal only.',
    wayland: 'This Linux desktop uses Wayland, which is not supported yet. Terminal only.',
    desktop_unsupported: 'Screen sharing is not supported on this device yet. Terminal only.',
    off: 'Remote access is switched off on this device.',
  };
  const SHORT = { needs_permission: 'Needs permission', locked: 'Locked', no_display: 'Terminal only', wayland: 'Terminal only', desktop_unsupported: 'Terminal only', off: 'Off' };
  const NO_SCREEN = { no_display: 1, wayland: 1, desktop_unsupported: 1 };
  // bye codes: the viewer's own sentence; anything else falls back to the host's "why"
  const BYE = {
    auth: 'This computer is no longer paired with the device. Pair again.',
    removed: 'This computer is no longer paired with the device. Pair again.',
    stopped: 'Stopped at the device.',
    timeout: 'The device stopped answering. Try again.',
    network: 'The network connection to the device dropped.',
    offline: 'The device is offline.',
    old: 'The app on the device is too old for remote control. Update it there.',
  };
  const FINAL = { off: 1, auth: 1, removed: 1, old: 1 }; // no Reconnect for these
  const SHAPES = new Set(['default', 'text', 'pointer', 'move', 'ew-resize', 'ns-resize', 'nwse-resize', 'nesw-resize', 'wait', 'crosshair', 'not-allowed']);
  const MODS = /^(Control|Alt|Shift|Meta)/;
  const ALL = 255;
  const MAC = /mac/i.test((navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || '');
  const BACKOFF = [1, 2, 5, 5, 5];
  const SENTINEL = '​'; // keeps the hidden phone input non-empty so Backspace still fires an input event
  // ponytail: escapes split across two 0x03 chunks show as stray text; buffer partial escapes if that shows up in practice.
  // groups: 1 CSI params, 2 CSI final, 3 handled control char, 4 printable run. Other escapes and controls are dropped.
  const TERM_RE = /\x1b\[([0-?]*)[ -\/]*([@-~])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[()][0-9A-Za-z]|\x1b[@-Z\\-_=>]|([\n\r\b\t])|[\x00-\x1f\x7f]|([^\x00-\x1f\x7f]+)/g;
  const TERM_KEYS = { Enter: '\r', Backspace: '\x7f', Tab: '\t', Escape: '\x1b', ArrowUp: '\x1b[A', ArrowDown: '\x1b[B', ArrowRight: '\x1b[C', ArrowLeft: '\x1b[D', Home: '\x1b[H', End: '\x1b[F', Delete: '\x1b[3~', PageUp: '\x1b[5~', PageDown: '\x1b[6~' };

  // 24x24 icons, 1.5 stroke (stroke width set in CSS).
  const P = {
    screen: 'M3 4.5h18v11H3zM8.5 20h7M12 15.5V20',
    quality: 'M4 7h9M17 7h3M4 17h3M11 17h9M15 5v4M9 15v4',
    eye: 'M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12zM12 9.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z',
    keys: 'M3 6.5h18v11H3zM7 10.5h.01M10.33 10.5h.01M13.67 10.5h.01M17 10.5h.01M8 14h8',
    term: 'M3.5 5h17v14h-17zM7.5 9.5l2.5 2.5-2.5 2.5M12.5 14.5h4',
    full: 'M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5',
    unfull: 'M9 4v5H4M15 4v5h5M15 20v-5h5M9 20v-5H4',
    power: 'M12 3.5v8M7 6.5a7.5 7.5 0 1 0 10 0',
    x: 'M6 6l12 12M18 6L6 18',
    pad: 'M4 4.5h16v15H4zM4 15h16M12 15v4.5',
    hand: 'M9 12V5.5a1.5 1.5 0 0 1 3 0V11M12 10.5v-1a1.5 1.5 0 0 1 3 0v1.5M15 10.5a1.5 1.5 0 0 1 3 0V15a6 6 0 0 1-6 6h-.5a6 6 0 0 1-4.9-2.5L4.3 15.3a1.5 1.5 0 0 1 2.4-1.8L9 16',
    up: 'M12 19V5M6 11l6-6 6 6',
    down: 'M12 5v14M6 13l6 6 6-6',
    left: 'M19 12H5M11 6l-6 6 6 6',
    right: 'M5 12h14M13 6l6 6-6 6',
    cursor: 'M5.5 3.5l13 7.4-5.6 1.4-2.8 5.4z',
    folder: 'M3 6.5a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z',
    file: 'M6.5 3.5h7l4 4v13h-11zM13.5 3.5v4h4',
    dl: 'M12 4v11M7 10l5 5 5-5M5 20h14',
    ul: 'M12 16V5M7 10l5-5 5 5M5 20h14',
    spk: 'M4 9.5h3.5L12 5.5v13l-4.5-4H4zM15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11',
    mute: 'M4 9.5h3.5L12 5.5v13l-4.5-4H4zM16 9.5l5 5M21 9.5l-5 5',
    chev: 'M8 10l4 4 4-4',
  };
  // files (cap "files")
  const CHUNK = 256 * 1024, AHEAD = 4 * 1024 * 1024, GB = 1024 * 1024 * 1024, MAX_FILE = 4 * GB, BLOB_MAX = GB;
  const TOO_BIG = 'Too large for this browser. Use a desktop Chromium browser.';
  const OVER_4G = 'Too large. The limit is 4 GB per file.';
  const fmtB = n => (n = Math.max(0, n || 0), n < 1024 ? Math.round(n) + ' B' : n < 1048576 ? (n / 1024).toFixed(n < 10240 ? 1 : 0) + ' KB' : n < GB ? (n / 1048576).toFixed(1) + ' MB' : (n / GB).toFixed(2) + ' GB');
  // sound (cap "audio"): worklet that plays posted Float32 chunks, starts after about 100 ms is buffered, drops past 300 ms
  const WORKLET = `registerProcessor('rv-pcm',class extends AudioWorkletProcessor{
constructor(){super();this.q=[];this.n=0;this.o=0;this.on=false;
this.port.onmessage=e=>{const d=e.data,k=d[0].length;if(this.n+k>sampleRate*.3)return;this.q.push(d);this.n+=k}}
process(_,outs){const out=outs[0],len=out[0].length;if(!this.on){if(this.n<sampleRate*.1)return true;this.on=true}
let i=0;while(i<len&&this.q.length){const c=this.q[0],k=Math.min(len-i,c[0].length-this.o);
for(let j=0;j<out.length;j++)out[j].set(c[Math.min(j,c.length-1)].subarray(this.o,this.o+k),i);
i+=k;this.o+=k;this.n-=k;if(this.o>=c[0].length){this.q.shift();this.o=0}}
if(i<len)this.on=false;return true}})`;

  const h = (tag, props, ...kids) => {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(props || {})) {
      if (k === 'text') e.textContent = v;
      else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
      else e.setAttribute(k, v);
    }
    e.append(...kids);
    return e;
  };
  const icon = name => {
    const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    s.setAttribute('viewBox', '0 0 24 24');
    s.setAttribute('aria-hidden', 'true');
    p.setAttribute('d', P[name]);
    s.append(p);
    return s;
  };
  const btn = (label, ico, onclick, cls) =>
    h('button', { type: 'button', class: 'rv-btn' + (cls ? ' ' + cls : ''), 'aria-label': label, title: label, onclick }, ico ? icon(ico) : '');
  const tbtn = (text, onclick, cls) => h('button', { type: 'button', class: 'rv-btn rv-text' + (cls ? ' ' + cls : ''), text, onclick });
  const setIcon = (b, name) => b.replaceChildren(icon(name));
  const label = (b, text) => { b.setAttribute('aria-label', text); b.title = text; };
  const clamp = v => Math.min(1, Math.max(0, v));
  const r4 = v => Math.round(clamp(v) * 10000) / 10000;
  const num = v => (typeof v === 'number' && isFinite(v) ? v : null);
  // a chunk never splits a surrogate pair
  function chunks(s, n) {
    const out = [];
    for (let i = 0; i < s.length;) {
      let j = Math.min(i + n, s.length);
      if (j < s.length && /[\ud800-\udbff]/.test(s[j - 1])) j--;
      out.push(s.slice(i, j));
      i = j;
    }
    return out;
  }
  const parseScreens = a => (Array.isArray(a) ? a.filter(x => x && Number.isInteger(x.id)) : []);
  // localStorage may be off (private mode, sandboxed webview): settings then last for the session only
  const store = (k, v) => { try { if (v === undefined) return localStorage.getItem(k); localStorage.setItem(k, String(v)); } catch {} return null; };
  const QN = { auto: 'Auto', speed: 'Speed', quality: 'Quality' }, FPS = [15, 30, 60];
  // JPEG frames are opaque and carry no color profile: skip both conversions (a browser that rejects the options decodes without)
  const BMP = { premultiplyAlpha: 'none', colorSpaceConversion: 'none' };
  const ms = v => (v < 10 ? v.toFixed(1) : Math.round(v)) + ' ms';
  // H.264 (cap "h264"): baseline Annex B, SPS and PPS in band, so the decoder is configured without a description
  const AVC = { codec: 'avc1.42E01F', optimizeForLatency: true };
  const hex2 = x => x.toString(16).padStart(2, '0');
  // the decoder says when it takes a chunk (dequeue event); without that, a chunk counts as taken once handed over
  const DEQ = typeof VideoDecoder === 'function' && 'ondequeue' in VideoDecoder.prototype;
  // the SPS NAL of an Annex B access unit (null if none); a keyframe with a new one configures the decoder again
  function spsOf(d) {
    for (let i = 0; i + 6 < d.length; i++) {
      if (d[i] || d[i + 1] || d[i + 2] !== 1 || (d[i + 3] & 31) !== 7) continue;
      let j = i + 4;
      while (j + 2 < d.length && !(d[j] === 0 && d[j + 1] === 0 && d[j + 2] <= 1)) j++;
      return d.subarray(i + 3, j);
    }
    return null;
  }

  function open({ mount, url, auth, title = 'Remote device', onClose }) {
    let ws = null, hello = null, conn = 'connecting', state = '', canInput = false, viewOnly = false, closed = false, bye = null;
    let proven = false, maxFps = FPS.includes(+store('rv-fps')) ? +store('rv-fps') : 60, statsOn = store('rv-stats') === '1';
    let tries = 0, retryT = 0, pingT = 0, pingN = 0, hideT = 0, raf = 0, resizeT = 0, lastRx = 0, lastPing = 0;
    let resume = false, takeover = false, first = true, caps = new Set(), stats = null, cmdCtrl = false, cmdPref = null, vHeld = false;
    let screen = null, quality = 'auto', fit = true, zoom = 1, iw = 0, ih = 0, frames = 0, fps = null, rtt = null, fpsT = performance.now();
    // frames: one decoding, one waiting (pend), one decoded waiting for the next paint (next); dec*/ack* sum over a 2 s ping window
    let pend = null, decoding = false, next = null, paintT = 0, lastAck = 0, decT = 0, decN = 0, ackT = 0, ackN = 0, dms = null, ams = null;
    // H.264: every chunk is decoded in order (only drawing is latest-wins); after an error deltas wait for a keyframe
    // qAck: chunks sent to the decoder and not acked yet (acked once the decoder takes them, so a decoder that holds
    // frames back never stalls the host's window); keyT: when a keyframe was last asked for; vErrs: errors since a picture
    let h264Ok = false, vd = null, vdSps = '', qAck = 0, vts = 0, needKey = true, keyT = 0, vErrs = 0;
    const vt0 = new Map(); // chunk timestamp -> when it went into the decoder
    let mmPending = null, termOpen = false, termHead = '', termLine = '', termCol = 0, termDirty = false, termDims = '';
    let mode = 'trackpad', touched = false, kbTarget = 'screen', g = null, lastTap = { t: 0, x: 0, y: 0 }, menuEl = null, menuBtn = null;
    let cur = { x: 0.5, y: 0.5 }, rcur = { x: 0.5, y: 0.5 }, curVis = true, curShape = 'default', lastLocal = 0;
    const wheel = { dx: 0, dy: 0 }, keys = new Set(), buttons = new Set(), pings = new Map(), touches = new Map();
    const sticky = new Set();
    let filesOpen = false, fsPath = '', fsParent = '', fsWait = 0, fsT = 0, fsData = null, xid = 0, xfT = 0, au = null, audioOn = false, vol = 0.8;
    let toastT = 0, noteSeen = '';
    const xfers = new Map(); // id -> transfer, in start order
    const AC = window.AudioContext || window.webkitAudioContext;
    const dec = new TextDecoder();
    let termDec = new TextDecoder(); // own decoder: terminal chunks may end mid character
    const coarse = matchMedia('(pointer: coarse)');

    // ---- DOM ----
    const canvas = h('canvas', { class: 'rv-canvas', tabindex: '0', role: 'img', 'aria-label': 'Remote screen of ' + title + '. Keys are sent to the device while focused.' });
    canvas.style.opacity = '0'; // an opaque canvas is a black box until the first frame sizes it
    const ctx = canvas.getContext('2d', { alpha: false, desynchronized: true });
    const cursorEl = h('div', { class: 'rv-cursor', hidden: '' }, icon('cursor'));
    const stage = h('div', { class: 'rv-stage' }, canvas, cursorEl);
    const spinner = h('div', { class: 'rv-spin', 'aria-hidden': 'true' });
    const msgEl = h('p', { class: 'rv-msg' }), subEl = h('p', { class: 'rv-sub' });
    const bAct = tbtn('', () => (bAct.dataset.act === 'close' ? disconnect() : reconnect(bAct.dataset.act === 'takeover')), 'rv-primary');
    const bAlt = tbtn('Close', () => disconnect(), 'rv-secondary');
    const panel = h('div', { class: 'rv-panel', role: 'status', 'aria-live': 'polite' }, spinner, msgEl, subEl, h('div', { class: 'rv-acts' }, bAct, bAlt));
    const pasteIn = h('textarea', { class: 'rv-paste-in', rows: '4', spellcheck: 'false', 'aria-label': 'Text to type on the remote device', placeholder: 'Paste or type text here' });
    const sheet = h('div', { class: 'rv-sheet', role: 'dialog', 'aria-label': 'Paste text', hidden: '' },
      h('p', { class: 'rv-sheet-t', text: 'Paste text' }), pasteIn,
      h('div', { class: 'rv-acts' }, tbtn('Cancel', () => closeSheet(), 'rv-secondary'), tbtn('Send', () => { sendText(pasteIn.value); closeSheet(); }, 'rv-primary')));
    // a short notice over the picture (sound failed, the device's note); text only ever goes in through textContent
    const toastText = h('span');
    const toastEl = h('div', { class: 'rv-toast', role: 'status', hidden: '' }, toastText, btn('Dismiss', 'x', () => toast('')));
    const statsEl = h('div', { class: 'rv-stats', 'aria-hidden': 'true', hidden: '' });
    const view = h('div', { class: 'rv-view' }, stage, statsEl, panel, sheet, toastEl);

    const termOut = h('pre', { class: 'rv-term-out', tabindex: '0', 'aria-label': 'Terminal. Type to send keys.' });
    const termTitle = h('span', { text: 'Terminal' });
    const term = h('section', { class: 'rv-term', hidden: '', 'aria-label': 'Terminal' },
      h('div', { class: 'rv-term-head' }, termTitle, btn('Close terminal', 'x', () => toggleTerm(false))), termOut);

    const titleEl = h('span', { class: 'rv-title', text: title, title });
    const dot = h('i', { class: 'rv-dot' }), chipText = h('span');
    const chip = h('span', { class: 'rv-chip' }, dot, chipText);
    const bFit = h('button', { type: 'button', class: 'rv-btn rv-text', onclick: () => { fit = !fit; zoom = 1; layout(); render(); } });
    const bScreen = btn('Switch screen', 'screen', () => menu(bScreen, screenItems));
    const qLbl = h('span');
    const bQual = btn('Stream quality', 'quality', () => menu(bQual, qualItems), 'rv-text');
    bQual.append(qLbl);
    const bView = btn('View only', 'eye', toggleView);
    const bKeys = btn('Send keys', 'keys', () => menu(bKeys, keyItems));
    const bTerm = btn('Terminal', 'term', () => toggleTerm());
    const bFiles = btn('Files', 'folder', () => toggleFiles());
    const bAudio = btn('Sound', 'mute', toggleAudio);
    const volIn = h('input', { type: 'range', class: 'rv-range', min: '0', max: '100', step: '5', value: '80', 'aria-label': 'Volume',
      oninput: () => { vol = volIn.value / 100; if (au) au.gain.gain.value = vol; } });
    const bVol = btn('Volume', 'chev', () => menu(bVol, () => h('div', { class: 'rv-menu rv-pop', role: 'group', 'aria-label': 'Volume' }, h('span', { class: 'rv-pop-t', text: 'Volume' }), volIn)), 'rv-vol');
    const bFull = btn('Fullscreen', 'full', toggleFull);
    const bBye = btn('Disconnect', 'power', disconnect, 'rv-danger');
    for (const b of [bScreen, bQual, bKeys, bVol]) { b.setAttribute('aria-haspopup', b === bVol ? 'true' : 'menu'); b.setAttribute('aria-expanded', 'false'); }
    for (const b of [bView, bTerm, bFiles, bAudio]) b.setAttribute('aria-pressed', 'false');
    const bar = h('div', { class: 'rv-bar', role: 'toolbar', 'aria-label': 'Remote session' },
      h('div', { class: 'rv-id' }, titleEl, chip),
      h('div', { class: 'rv-actions' }, bFit, bScreen, bQual, bView, bKeys, bTerm, bFiles, bAudio, bVol, bFull), bBye);
    // a mouse click must not move focus off the canvas (keys would stop reaching the remote, Space would re-press the button)
    bar.addEventListener('mousedown', e => { if (e.target.closest('.rv-btn')) e.preventDefault(); });

    const kbIn = h('input', { class: 'rv-kbin', type: 'text', autocomplete: 'off', autocapitalize: 'off', autocorrect: 'off', spellcheck: 'false', enterkeyhint: 'send', 'aria-label': 'Type to the remote device' });
    const bMode = btn('Direct touch', 'pad', () => {
      mode = mode === 'trackpad' ? 'direct' : 'trackpad';
      bMode.setAttribute('aria-pressed', String(mode === 'direct'));
      setIcon(bMode, mode === 'direct' ? 'hand' : 'pad');
      placeCursor();
    });
    bMode.setAttribute('aria-pressed', 'false');
    const bKb = btn('Keyboard', 'keys', () => { kbIn.value = SENTINEL; kbIn.focus(); });
    const stickyBtn = (text, code) => {
      const b = h('button', { type: 'button', class: 'rv-btn rv-text', 'aria-pressed': 'false', 'aria-label': text + ' (held for the next key)', title: text, text,
        onclick: () => { sticky.has(code) ? sticky.delete(code) : sticky.add(code); b.setAttribute('aria-pressed', String(sticky.has(code))); } });
      b.dataset.code = code;
      return b;
    };
    const mods = [stickyBtn('Ctrl', 'ControlLeft'), stickyBtn('Alt', 'AltLeft'), stickyBtn('Shift', 'ShiftLeft')];
    const special = (lbl, code, ico) => {
      const b = ico ? btn(lbl, ico, () => tapKey(code)) : h('button', { type: 'button', class: 'rv-btn rv-text', 'aria-label': lbl, title: lbl, text: lbl, onclick: () => tapKey(code) });
      b.addEventListener('pointerdown', e => e.preventDefault()); // keep the phone keyboard open
      return b;
    };
    for (const b of [...mods, bKb]) b.addEventListener('pointerdown', e => e.preventDefault());
    const tbar = h('div', { class: 'rv-touchbar', role: 'toolbar', 'aria-label': 'Touch controls' },
      bMode, bKb, h('i', { class: 'rv-sep' }), ...mods,
      special('Esc', 'Escape'), special('Tab', 'Tab'),
      special('Left arrow', 'ArrowLeft', 'left'), special('Up arrow', 'ArrowUp', 'up'), special('Down arrow', 'ArrowDown', 'down'), special('Right arrow', 'ArrowRight', 'right'),
      kbIn);

    // files panel: side panel, a bottom sheet on narrow screens. Remote names and paths only ever go in through textContent.
    const fileIn = h('input', { type: 'file', multiple: '', hidden: '', tabindex: '-1', onchange: () => { queueUp([...fileIn.files]); fileIn.value = ''; } });
    const bSend = h('button', { type: 'button', class: 'rv-btn rv-text rv-secondary', onclick: () => fileIn.click() }, icon('ul'), 'Send files');
    const bUp = btn('Up one folder', 'up', () => fsParent && list(fsParent));
    const crumbs = h('nav', { class: 'rv-crumbs', 'aria-label': 'Folder' });
    const fNav = h('div', { class: 'rv-fnav' }, bUp, crumbs);
    const fNote = h('p', { class: 'rv-fnote' });
    const fMsg = h('p', { class: 'rv-fmsg', role: 'status' });
    const fList = h('ul', { class: 'rv-flist', 'aria-label': 'Folder contents' });
    const fBody = h('div', { class: 'rv-fbody' }, fMsg, fList);
    const xHead = h('p', { class: 'rv-xhead', text: 'Transfers', hidden: '' });
    const xList = h('ul', { class: 'rv-xlist', 'aria-label': 'Transfers', hidden: '' });
    const files = h('section', { class: 'rv-files', tabindex: '-1', 'aria-label': 'Files on ' + title, hidden: '' },
      h('div', { class: 'rv-files-head' }, h('span', { class: 'rv-sheet-t', text: 'Files' }), bSend, btn('Close files', 'x', () => toggleFiles(false))),
      fNote, fNav, fBody, xHead, xList, fileIn);
    // Esc inside the panel closes it and never reaches the remote
    files.addEventListener('keydown', e => { if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); toggleFiles(false, true); } });

    const root = h('div', { class: 'rv' }, bar, view, term, tbar, files);
    mount.append(root);

    // ---- connection ----
    const live = () => !!(hello && state === 'ready' && conn === 'open');
    const can = () => live() && canInput && !viewOnly;
    const send = o => { if (ws && ws.readyState === 1 && hello) ws.send(JSON.stringify(o)); };
    const start = () => {
      const o = { t: 'start', screen, quality, maxFps };
      if (caps.has('cursor')) o.cursor = true;
      if (h264Ok && caps.has('h264')) o.codecs = ['h264'];
      send(o);
    };
    // offer H.264 only when this browser says it decodes it; a session already streaming JPEG switches over
    if (typeof VideoDecoder === 'function') {
      try {
        VideoDecoder.isConfigSupported(AVC).then(r => { if (!closed && r && r.supported) { h264Ok = true; if (live() && caps.has('h264')) start(); } }, () => {});
      } catch {}
    }
    const allOk = () => !!hello && caps.has('all') && hello.screens.length > 1;
    const screenOk = id => !!hello && (hello.screens.some(x => x.id === id) || (id === ALL && allOk()));
    const mainScreen = () => (hello.screens.find(x => x.primary) || hello.screens[0] || { id: 0 }).id;
    // toast(msg, ms): shows msg until dismissed, or for ms; toast('') hides it
    function toast(msg, ms) {
      clearTimeout(toastT);
      toastText.textContent = msg;
      toastEl.hidden = !msg;
      if (msg && ms) toastT = setTimeout(() => toast(''), ms);
    }

    function connect() {
      clearTimeout(retryT);
      conn = 'connecting';
      render();
      let u = url, s;
      if (takeover) try { const x = new URL(url, location.href); x.searchParams.set('takeover', '1'); u = x.href; } catch {}
      try { s = new WebSocket(u); } catch { return lost(null); }
      ws = s;
      s.binaryType = 'arraybuffer';
      s.onopen = () => { if (s === ws && auth) s.send(JSON.stringify(takeover ? { t: 'auth', token: auth, takeover: true } : { t: 'auth', token: auth })); };
      s.onmessage = e => { if (s === ws) { lastRx = performance.now(); onMsg(e.data); } };
      s.onclose = () => lost(s);
    }
    // user pressed Reconnect or Take over
    function reconnect(tk) {
      bye = null;
      state = '';
      tries = 0;
      resume = false;
      takeover = tk;
      connect();
    }

    function lost(s) {
      if (s && s !== ws) return;
      const had = !!hello;
      releaseAll();
      hello = null;
      caps = new Set();
      stats = null;
      pend = null;
      vreset();
      termDec = new TextDecoder();
      clearInterval(pingT);
      pings.clear();
      if (termOpen) toggleTerm(false);
      closeSheet(true);
      // transfers are not resumed; sound pauses and comes back after a reconnect if it was on
      for (const t of xfers.values()) end(t, 'err', 'Stopped. The connection to the device dropped.');
      fsWait = 0;
      clearTimeout(fsT);
      fsData = null;
      if (au) { if (au.node) au.node.disconnect(); au.node = null; au.rate = 0; au.ctx.suspend().catch(() => {}); }
      if (closed) return;
      if (bye !== null || state === 'off') { conn = 'ended'; return render(); }
      if (had) resume = true;
      // only a pong or a drawn frame proves the link works; hello alone does not, or a slow link retries forever
      if (proven) tries = 0;
      proven = false;
      if (tries >= BACKOFF.length) { conn = 'failed'; return render(); }
      conn = 'retry';
      retryT = setTimeout(connect, BACKOFF[tries++] * 1000);
      render();
    }
    // drop a socket that went silent and go through the normal reconnect path
    function dead() {
      const s = ws;
      if (!s) return;
      s.onclose = s.onmessage = null;
      try { s.close(); } catch {}
      lost(s); // the reconnect keeps the user's quality and frame rate
    }

    function onMsg(data) {
      if (typeof data === 'string') return; // downstream is binary only
      const b = new Uint8Array(data);
      if (b[0] === 2) {
        let m;
        try { m = JSON.parse(dec.decode(b.subarray(1))); } catch { return; }
        if (m && typeof m === 'object') onCtl(m);
      } else if (!hello) return;
      else if (b[0] === 1 && b.length > 2) frame(b);
      else if (b[0] === 6 && b.length > 3) vframe(b);
      else if (b[0] === 3) termWrite(b.subarray(1));
      else if (b[0] === 4 && b.length > 5) fileChunk(b);
      else if (b[0] === 5) pcm(b);
    }

    function setState(m) {
      const was = state;
      state = typeof m.state === 'string' ? m.state : 'ready';
      canInput = m.canInput !== false;
      if (!canInput) releaseAll();
      if (state === 'ready' && was !== 'ready' && hello && !document.hidden) start();
      if (NO_SCREEN[state] && hello && hello.terminal && !termOpen) toggleTerm(true);
    }

    function onCtl(m) {
      if (m.t === 'bye') {
        const why = String(m.why || '');
        const code = typeof m.code === 'string' ? m.code : /already connected/i.test(why) ? 'busy' : '';
        // our own stale session still holds the slot after a drop: keep backing off instead of ending
        if (code === 'busy' && resume && !hello && tries < BACKOFF.length) { try { ws.close(); } catch {} return; }
        bye = { code, why, by: typeof m.by === 'string' ? m.by : '', had: !!hello };
        conn = 'ended';
        render();
        try { ws.close(); } catch {}
        return;
      }
      if (m.t === 'hello') {
        if (hello) return;
        caps = new Set(Array.isArray(m.caps) ? m.caps.map(String) : []);
        hello = { os: String(m.os || ''), screens: parseScreens(m.screens), terminal: !!m.terminal, combos: Array.isArray(m.combos) ? m.combos.map(String) : [],
          note: typeof m.note === 'string' ? m.note.slice(0, 300) : '' };
        if (hello.note && hello.note !== noteSeen) { noteSeen = hello.note; toast(hello.note); } // once, not on every reconnect
        if (!screenOk(screen)) screen = Number.isInteger(m.screen) && screenOk(m.screen) ? m.screen : mainScreen();
        cmdCtrl = cmdPref != null ? cmdPref : MAC && hello.os !== 'macos';
        conn = 'open';
        takeover = false;
        state = '';
        stats = null;
        fps = rtt = dms = ams = null;
        lastAck = 0;
        lastPing = 0;
        setState(m);
        if (viewOnly) send({ t: 'view', on: true });
        if (audioOn) { if (au && caps.has('audio')) { au.ctx.resume().catch(() => {}); send({ t: 'audio', on: true }); } else audioStop(false); }
        clearInterval(pingT);
        pingT = setInterval(ping, 2000);
        render();
        armHide();
        const a = document.activeElement;
        if (!termOpen && (first || !a || a === document.body)) canvas.focus({ preventScroll: true });
        first = false;
        return;
      }
      if (!hello) return; // nothing but hello and bye before hello
      if (m.t === 'state') { setState(m); render(); }
      else if (m.t === 'pong' && pings.has(m.n)) { proven = true; rtt = Math.round(performance.now() - pings.get(m.n)); pings.delete(m.n); renderChip(); }
      else if (m.t === 'screens' && caps.has('screens')) {
        hello.screens = parseScreens(m.screens);
        if (Number.isInteger(m.screen) && screenOk(m.screen)) screen = m.screen;
        else if (!screenOk(screen)) { screen = mainScreen(); start(); }
        if (menuBtn === bScreen) { closeMenu(); menu(bScreen, screenItems); }
        render();
      } else if (m.t === 'cur' && caps.has('cursor')) {
        rcur = { x: clamp(num(m.x) || 0), y: clamp(num(m.y) || 0) };
        curVis = m.vis !== false;
        curShape = SHAPES.has(m.shape) ? m.shape : 'default';
        if (performance.now() - lastLocal > 300) cur = rcur; // the user is not moving: trust where the device says it is
        placeCursor();
      } else if (m.t === 'stats' && caps.has('stats')) {
        stats = { q: num(m.q), scale: num(m.scale), fps: num(m.fps), kbps: num(m.kbps), cap: num(m.cap), enc: num(m.enc), kb: num(m.kb), inflight: num(m.inflight), w: num(m.w),
          codec: m.codec === 'h264' ? 'H.264' : m.codec === 'jpeg' ? 'JPEG' : null, br: num(m.br),
          encoder: typeof m.encoder === 'string' ? m.encoder.slice(0, 60) : null };
        renderChip();
      } else if (m.t === 'fs' && caps.has('files')) onList(m);
      else if (m.t === 'file' && caps.has('files')) onFile(m);
      else if (m.t === 'audio' && caps.has('audio')) {
        if (m.on !== false) return audioFormat(m);
        if (audioOn) audioStop(false); // the device could not (or no longer) send sound
        if (m.err != null) toast(String(m.err) || 'Sound stopped on the device.', 8000);
      }
    }

    function ping() {
      const now = performance.now();
      // nothing heard for 8 s while pings went out on time: the link is dead even if TCP has not noticed yet
      if (now - lastRx > 8000 && lastPing && now - lastPing < 4000) return dead();
      fps = Math.round(frames * 1000 / (now - fpsT));
      frames = 0;
      fpsT = now;
      dms = decN ? decT / decN : null;
      ams = ackN ? ackT / ackN : null;
      decT = decN = ackT = ackN = 0;
      lastPing = now;
      pings.set(++pingN, now);
      if (pings.size > 5) pings.delete(pings.keys().next().value);
      send({ t: 'ping', n: pingN });
      renderChip();
    }

    // latest frame wins: one decode at a time and one frame waiting; a newer frame replaces the waiting one, which is
    // still acked so the host's window opens. Ack once decoded (not drawn); draw on the next animation frame.
    // ack ms: from a decoded frame's ack to the next frame's arrival (about the round trip while the host waits on acks)
    const ack = timed => { lastAck = timed ? performance.now() : 0; send({ t: 'ack' }); };
    function frame(b) {
      if (lastAck) { ackT += performance.now() - lastAck; ackN++; lastAck = 0; }
      if (pend) ack();
      pend = b;
      decode();
    }
    function decode() {
      if (decoding || !pend) return;
      const b = pend;
      pend = null;
      if (b[1] !== screen) return ack(); // another screen (or one switched away from while waiting)
      const blob = new Blob([b.subarray(2)], { type: 'image/jpeg' }), t0 = performance.now();
      decoding = true;
      createImageBitmap(blob, BMP).catch(() => createImageBitmap(blob))
        .then(bmp => {
          if (closed) return bmp.close();
          decT += performance.now() - t0;
          decN++;
          proven = true;
          if (next) next.close();
          next = bmp;
          if (!paintT) paintT = requestAnimationFrame(paint);
        }, () => {})
        .then(() => { decoding = false; if (!closed) { ack(true); decode(); } });
    }

    // H.264 frame: 0x06, screen, flags (bit 0 keyframe), Annex B access unit. Never skipped while the decoder is
    // healthy (each delta needs the one before); dropped ones (another screen, waiting for a keyframe) are acked at once.
    function vframe(b) {
      if (lastAck) { ackT += performance.now() - lastAck; ackN++; lastAck = 0; }
      const key = (b[2] & 1) === 1, d = b.subarray(3);
      if (!h264Ok || b[1] !== screen) { needKey = true; return ack(); } // a new stream starts with a keyframe
      if (needKey && !key) { askKey(); return ack(); }
      try {
        if (!vd) {
          // callbacks of a decoder that was replaced do nothing
          const me = new VideoDecoder({ output: f => (me === vd ? vout(f) : f.close()), error: e => { if (me === vd) vfail(e); } });
          me.ondequeue = () => { if (me === vd) vtaken(); };
          vd = me;
        }
        if (key) {
          const sps = spsOf(d), id = sps ? sps.join() : vdSps;
          if (vd.state !== 'configured' || id !== vdSps) {
            vd.configure(sps && sps.length > 3 ? { ...AVC, codec: 'avc1.' + hex2(sps[1]) + hex2(sps[2]) + hex2(sps[3]) } : AVC);
            vdSps = id;
          }
          needKey = false;
          keyT = 0;
        }
        vd.decode(new EncodedVideoChunk({ type: key ? 'key' : 'delta', timestamp: ++vts, data: d }));
        qAck++;
        vt0.set(vts, performance.now());
        vtaken();
      } catch (e) { ack(); vfail(e); }
    }
    // ack every chunk the decoder has taken (its queue is shorter than what is unacked)
    function vtaken() {
      const left = DEQ && vd ? vd.decodeQueueSize : 0;
      while (qAck > left) { qAck--; ack(true); }
    }
    function vout(f) {
      const t0 = vt0.get(f.timestamp);
      vt0.delete(f.timestamp);
      if (closed) return f.close();
      if (t0 != null) { decT += performance.now() - t0; decN++; }
      proven = true;
      vErrs = 0;
      if (next) next.close();
      next = f;
      if (!paintT) paintT = requestAnimationFrame(paint);
    }
    // ask for a keyframe; again after a second while still waiting (the host may have been busy)
    function askKey() {
      needKey = true;
      const now = performance.now();
      if (now - keyT > 1000) { keyT = now; send({ t: 'idr' }); }
    }
    function vreset() {
      try { if (vd && vd.state !== 'closed') vd.close(); } catch {}
      vd = null;
      vdSps = '';
      qAck = 0;
      vt0.clear();
      needKey = true;
      keyT = 0;
    }
    // decode error: a fresh decoder from the next keyframe; chunks it never took are acked so the host's window opens.
    // A configuration this browser cannot decode after all (or rejects), or 3 errors without a picture: back to JPEG.
    function vfail(e) {
      const n = qAck;
      vreset();
      for (let i = 0; i < n; i++) ack();
      if (closed) return;
      if (++vErrs >= 3 || (e && (e.name === 'NotSupportedError' || e.name === 'TypeError'))) { h264Ok = false; vErrs = 0; if (live()) start(); }
      else if (live()) askKey();
    }
    function paint() {
      paintT = 0;
      const bmp = next;
      next = null;
      if (!bmp) return;
      const w = bmp.displayWidth || bmp.width, h = bmp.displayHeight || bmp.height; // VideoFrame or ImageBitmap
      if (w !== iw || h !== ih) { iw = canvas.width = w; ih = canvas.height = h; canvas.style.opacity = ''; layout(); }
      ctx.drawImage(bmp, 0, 0);
      frames++;
      bmp.close();
    }

    // ---- layout ----
    function layout() {
      if (!iw) return;
      const base = fit ? Math.min(stage.clientWidth / iw, stage.clientHeight / ih) : 1 / (devicePixelRatio || 1);
      canvas.style.width = Math.round(iw * base * zoom) + 'px';
      canvas.style.height = Math.round(ih * base * zoom) + 'px';
      placeCursor();
    }
    const ro = new ResizeObserver(() => { layout(); termResize(); root.style.setProperty('--_barh', bar.offsetHeight + 'px'); });
    ro.observe(stage);
    ro.observe(termOut);
    ro.observe(bar);

    function placeCursor() {
      const c = caps.has('cursor'), trackpad = touched && mode === 'trackpad';
      canvas.style.cursor = c && can() ? (curVis ? curShape : 'none') : '';
      // without the cursor cap the pointer is in the picture; with it, draw it for trackpad users and when not controlling
      const show = iw > 0 && live() && (c ? curVis && (trackpad || !can()) : trackpad);
      cursorEl.hidden = !show;
      if (!show) return;
      const p = c && !can() ? rcur : cur;
      const x = canvas.offsetLeft + p.x * canvas.offsetWidth, y = canvas.offsetTop + p.y * canvas.offsetHeight;
      cursorEl.style.transform = `translate(${x}px,${y}px)`;
      if (zoom > 1 || !fit) { // keep the cursor in view while the picture overflows
        const m = 40;
        if (x < stage.scrollLeft + m) stage.scrollLeft = x - m;
        else if (x > stage.scrollLeft + stage.clientWidth - m) stage.scrollLeft = x - stage.clientWidth + m;
        if (y < stage.scrollTop + m) stage.scrollTop = y - m;
        else if (y > stage.scrollTop + stage.clientHeight - m) stage.scrollTop = y - stage.clientHeight + m;
      }
    }

    function zoomTo(z, mid) {
      z = Math.min(4, Math.max(1, z));
      if (z === zoom) return;
      const r = canvas.getBoundingClientRect(), sr = stage.getBoundingClientRect();
      const fx = (mid.x - r.left) / r.width, fy = (mid.y - r.top) / r.height;
      zoom = z;
      layout();
      stage.scrollLeft = canvas.offsetLeft + fx * canvas.offsetWidth - (mid.x - sr.left);
      stage.scrollTop = canvas.offsetTop + fy * canvas.offsetHeight - (mid.y - sr.top);
    }
    // 100%: the mouse position inside the stage picks which part of the picture is shown
    function pan(e) {
      const r = stage.getBoundingClientRect();
      stage.scrollLeft = clamp((e.clientX - r.left) / r.width) * (stage.scrollWidth - stage.clientWidth);
      stage.scrollTop = clamp((e.clientY - r.top) / r.height) * (stage.scrollHeight - stage.clientHeight);
    }

    // ---- render ----
    function render() {
      if (closed) return;
      let msg = '', sub = '', spin = false, act = '', alt = false;
      if (conn === 'ended') {
        const c = bye ? bye.code : 'off';
        alt = true;
        if (c === 'busy') { msg = bye.by ? 'In use by ' + bye.by + '.' : 'Someone else is connected.'; act = 'takeover'; }
        else if (c === 'replaced') { msg = 'Another session took over.'; act = 'retry'; }
        else if (c === 'off') { msg = TEXT.off; sub = bye && bye.why && bye.why !== TEXT.off ? bye.why : ''; }
        else { msg = bye.had ? 'The session was ended on the device.' : 'Could not start the session.'; sub = BYE[c] || bye.why; act = FINAL[c] ? '' : 'retry'; }
        if (!act) { act = 'close'; alt = false; }
      }
      else if (conn === 'failed') { msg = 'Could not reach ' + title + '.'; sub = 'Check that the device is on and connected, then try again.'; act = 'retry'; alt = true; }
      else if (conn === 'retry') { msg = 'Connection lost. Reconnecting.'; sub = `Attempt ${tries} of ${BACKOFF.length}`; spin = true; }
      else if (!hello) { msg = 'Connecting to ' + title; spin = true; }
      else if (state !== 'ready') msg = TEXT[state] || 'This device cannot be shown right now.';
      panel.hidden = !msg;
      msgEl.textContent = msg;
      subEl.textContent = sub;
      subEl.hidden = !sub;
      spinner.hidden = !spin;
      bAct.hidden = !act;
      bAct.dataset.act = act;
      bAct.textContent = act === 'close' ? 'Close' : act === 'takeover' ? 'Take over' : 'Reconnect';
      bAlt.hidden = !alt;
      root.classList.toggle('rv-noscreen', !!(hello && NO_SCREEN[state] && termOpen));
      root.classList.toggle('rv-live', live());

      const hasScreen = live();
      bFit.textContent = fit ? 'Fit' : '100%';
      label(bFit, fit ? 'Scaled to fit. Switch to 100%' : 'Shown at 100%. Switch to fit');
      bFit.disabled = bQual.disabled = !hasScreen;
      qLbl.textContent = QN[quality] + ' · ' + maxFps;
      label(bQual, `Stream: ${QN[quality]}, up to ${maxFps} fps`);
      bKeys.disabled = !can();
      bScreen.hidden = !(hello && hello.screens.length > 1);
      bScreen.disabled = !hasScreen;
      bView.disabled = !hello;
      bTerm.hidden = !(hello && hello.terminal);
      bTerm.disabled = viewOnly && !termOpen; // view only covers the terminal (the host refuses open and input)
      termTitle.textContent = viewOnly ? 'Terminal, view only' : 'Terminal';
      bFull.hidden = !(document.fullscreenEnabled || document.webkitFullscreenEnabled);
      bFiles.hidden = !(caps.has('files') || filesOpen);
      bAudio.hidden = bVol.hidden = !caps.has('audio');
      bAudio.disabled = !AC || !hello;
      bAudio.setAttribute('aria-pressed', String(audioOn));
      setIcon(bAudio, audioOn ? 'spk' : 'mute');
      bAudio.title = !AC ? 'Sound is not supported in this browser' : audioOn ? 'Sound is on. Turn off' : 'Sound is off. Turn on';
      renderFiles();
      if (!can()) closeSheet(true);
      renderChip();
      placeCursor();
      if (!hasScreen) showBar();
    }

    const rate = k => (k >= 1000 ? (k / 1000).toFixed(1) + ' Mbps' : Math.round(k) + ' kbps');
    function renderChip() {
      let t = 'Connecting', tone = '', tip = '';
      if (conn === 'ended') { t = 'Ended'; tone = 'bad'; }
      else if (conn === 'failed') { t = 'Offline'; tone = 'bad'; }
      else if (conn === 'retry') { t = 'Reconnecting'; tone = 'warn'; }
      else if (hello && state !== 'ready') { t = SHORT[state] || state; tone = 'warn'; }
      else if (hello) {
        const parts = [fps ? fps + ' fps' : 'Live']; // 0 fps with pongs arriving is a still screen, not a dead one
        if (stats && stats.kbps != null) parts.push(rate(stats.kbps));
        if (rtt != null) parts.push(rtt + ' ms');
        if (viewOnly || !canInput) parts.push('View only');
        t = parts.join(' · ');
        tone = 'ok';
        if (stats) {
          const s = [];
          if (stats.q != null) s.push('Quality ' + Math.round(stats.q));
          if (stats.br != null) s.push('H.264 up to ' + rate(stats.br) + (stats.encoder ? ' (' + stats.encoder + ')' : ''));
          if (stats.scale != null) s.push('scale ' + Math.round(stats.scale * 100) + '%');
          if (stats.fps != null) s.push(Math.round(stats.fps) + ' fps sent');
          if (stats.kbps != null) s.push(rate(stats.kbps));
          tip = s.join(', ') + (stats.scale != null && stats.scale < 1 ? '. Reduced quality (slow connection)' : '');
        }
      }
      chipText.textContent = t;
      dot.dataset.tone = tone;
      statsEl.hidden = !(statsOn && live());
      if (!statsEl.hidden) { // host fields only when the host sends them (older hosts send q, scale, fps, kbps or nothing)
        const st = stats || {};
        statsEl.textContent = [
          fps != null && `${fps}/${maxFps} fps`, st.fps != null && `sent ${Math.round(st.fps)}`, st.codec, st.encoder,
          st.kbps != null && rate(st.kbps) + (st.br != null ? ' of ' + rate(st.br) : ''),
          st.cap != null && 'cap ' + ms(st.cap), st.enc != null && 'enc ' + ms(st.enc), st.kb != null && Math.round(st.kb) + ' kb',
          st.inflight != null && 'inflight ' + st.inflight, st.w != null && 'w ' + st.w,
          dms != null && 'dec ' + ms(dms), ams != null && 'ack ' + ms(ams), rtt != null && 'rtt ' + rtt + ' ms',
        ].filter(Boolean).join(' · ');
      }
      if (hello && caps.has('secure')) tip = (tip ? tip.replace(/\.?$/, '. ') : '') + 'Lock screen control is on for this device.';
      if (hello && hello.note) tip = (tip ? tip + '\n' : '') + hello.note;
      if (tip) chip.title = tip; else chip.removeAttribute('title');
    }

    // ---- toolbar: in flow, so it never covers the picture; overlays and auto-hides only in fullscreen with a mouse ----
    const isFs = () => root.classList.contains('rv-fs');
    const showBar = () => { clearTimeout(hideT); hideT = 0; bar.classList.remove('rv-hide'); };
    const canHide = () => isFs() && !coarse.matches && live() && !menuEl && !bar.contains(document.activeElement);
    const hideBar = () => { clearTimeout(hideT); hideT = 0; if (canHide()) bar.classList.add('rv-hide'); };
    function armHide() {
      if (hideT || !canHide()) return;
      hideT = setTimeout(hideBar, 3000);
    }
    root.addEventListener('pointermove', e => {
      if (e.pointerType !== 'mouse' || !isFs()) return;
      const y = e.clientY - root.getBoundingClientRect().top;
      if (y < 4) showBar();
      else if (y > bar.offsetHeight) hideBar();
    });
    bar.addEventListener('focusin', showBar);
    bar.addEventListener('focusout', () => setTimeout(armHide));

    // ---- menus ----
    function screenItems() {
      const items = hello.screens.map((s, i) => ({ label: `Screen ${i + 1}` + (s.w ? ` (${s.w}x${s.h})` : '') + (s.primary ? ', main' : ''), checked: s.id === screen, run: () => { screen = s.id; start(); } }));
      if (allOk()) items.push({ label: 'All screens', checked: screen === ALL, run: () => { screen = ALL; start(); } });
      return items;
    }
    function qualItems() {
      const set = f => () => { f(); start(); render(); }; // start again mid session: the host retargets the stream
      return [
        ...Object.keys(QN).map(v => ({ label: QN[v], checked: quality === v, run: set(() => { quality = v; }) })),
        { sep: 'Frame rate' },
        ...FPS.map(n => ({ label: n + ' fps', checked: maxFps === n, run: set(() => { maxFps = n; store('rv-fps', n); }) })),
        { sep: '' },
        { label: 'Show stats', check: statsOn, run: () => { statsOn = !statsOn; store('rv-stats', statsOn ? 1 : 0); renderChip(); } },
      ];
    }
    function keyItems() {
      const mac = hello.os === 'macos', mod = mac ? 'MetaLeft' : 'ControlLeft', m = mac ? 'Cmd' : 'Ctrl', media = !caps.has('media');
      return [
        { label: 'Paste text', run: openSheet },
        { label: 'Ctrl+Alt+Del', hidden: !hello.combos.includes('cad'), run: () => can() && send({ t: 'combo', name: 'cad' }) },
        { label: 'Lock', run: () => can() && send({ t: 'combo', name: 'lock' }) },
        { label: 'Esc', run: () => chord('Escape') },
        { label: 'Tab', run: () => chord('Tab') },
        { label: mac ? 'Cmd' : 'Win', run: () => chord('MetaLeft') },
        { label: mac ? 'Cmd+Tab' : 'Alt+Tab', run: () => chord(mac ? 'MetaLeft' : 'AltLeft', 'Tab') },
        { label: m + '+C', run: () => chord(mod, 'KeyC') },
        { label: m + '+V', run: () => chord(mod, 'KeyV') },
        { label: 'Volume up', hidden: media, run: () => chord('AudioVolumeUp') },
        { label: 'Volume down', hidden: media, run: () => chord('AudioVolumeDown') },
        { label: 'Mute', hidden: media, run: () => chord('AudioVolumeMute') },
        { label: 'Play or pause', hidden: media, run: () => chord('MediaPlayPause') },
        { label: 'Cmd acts as Ctrl', hidden: !MAC, check: cmdCtrl, run: () => { releaseAll(); cmdCtrl = cmdPref = !cmdCtrl; } },
      ];
    }
    function menu(b, items) {
      const again = menuBtn === b;
      closeMenu();
      if (again || !hello) return;
      const got = items(), pop = got instanceof Element; // an element is a popover (volume), not a menu
      const m = pop ? got : h('div', { class: 'rv-menu', role: 'menu', 'aria-label': b.getAttribute('aria-label') });
      if (!pop) for (const it of got) {
        if (it.hidden) continue;
        if (it.sep != null) { m.append(h('p', { class: 'rv-mh', role: 'presentation', text: it.sep })); continue; }
        const role = it.check != null ? 'menuitemcheckbox' : it.checked != null ? 'menuitemradio' : 'menuitem';
        const mi = h('button', { type: 'button', class: 'rv-mi', role, tabindex: '-1', text: it.label,
          onclick: e => {
            const kb = e.detail === 0; // keyboard: focus back to the menu button; pointer: back to the remote screen
            closeMenu(kb);
            if (!kb && live() && !termOpen) canvas.focus({ preventScroll: true });
            it.run();
          } });
        if (role !== 'menuitem') mi.setAttribute('aria-checked', String(!!(it.check != null ? it.check : it.checked)));
        m.append(mi);
      }
      m.addEventListener('keydown', e => {
        const list = [...m.querySelectorAll('.rv-mi')], i = list.indexOf(document.activeElement);
        if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closeMenu(true); }
        else if (pop) { if (e.key === 'Tab') closeMenu(); }
        else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); list[(i + (e.key === 'ArrowDown' ? 1 : list.length - 1)) % list.length].focus(); }
        else if (e.key === 'Tab') closeMenu();
      });
      root.append(m);
      const r = b.getBoundingClientRect(), rr = root.getBoundingClientRect();
      m.style.top = r.bottom - rr.top + 6 + 'px';
      m.style.left = Math.max(8, Math.min(r.left - rr.left, rr.width - m.offsetWidth - 8)) + 'px';
      b.setAttribute('aria-expanded', 'true');
      menuEl = m;
      menuBtn = b;
      showBar();
      (m.querySelector('[role=menuitemradio][aria-checked=true]') || m.querySelector('button,input')).focus();
    }
    function closeMenu(refocus) {
      if (!menuEl) return;
      menuEl.remove();
      menuBtn.setAttribute('aria-expanded', 'false');
      if (refocus) menuBtn.focus();
      menuEl = menuBtn = null;
    }
    root.addEventListener('pointerdown', e => { if (menuEl && !menuEl.contains(e.target) && !menuBtn.contains(e.target)) closeMenu(); }, true);

    // ---- paste sheet (phones, webviews without clipboard permission) ----
    function openSheet() {
      if (!can()) return;
      pasteIn.value = '';
      sheet.hidden = false;
      pasteIn.focus();
    }
    function closeSheet(quiet) {
      if (sheet.hidden) return;
      sheet.hidden = true;
      if (!quiet && live() && !termOpen) canvas.focus({ preventScroll: true });
    }
    sheet.addEventListener('keydown', e => {
      if (e.key === 'Escape') { e.preventDefault(); closeSheet(); }
      else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); sendText(pasteIn.value); closeSheet(); }
    });

    // ---- files (cap "files"): browse, download, upload. A transfer ends with its socket; nothing is resumed. ----
    const filesOk = () => !closed && !!hello && conn === 'open' && caps.has('files') && !viewOnly && canInput;
    const busy = t => t.st === 'run' || t.st === 'queued';
    const join = (p, n) => (/[\\/]$/.test(p) ? p : p + (p.includes('\\') && !p.includes('/') ? '\\' : '/')) + n;
    function toggleFiles(on = !filesOpen, kb) {
      if (on === filesOpen) return;
      filesOpen = on;
      files.hidden = !on;
      bFiles.setAttribute('aria-pressed', String(on));
      if (on) { fsData = null; render(); files.focus({ preventScroll: true }); return; }
      if (files.contains(document.activeElement) || document.activeElement === document.body) (!kb && live() && !termOpen ? canvas : bFiles).focus({ preventScroll: true });
      render();
    }
    function renderFiles() {
      if (!filesOpen) return;
      const ok = filesOk();
      fNote.textContent = ok ? '' : !hello ? 'Not connected.' : !caps.has('files') ? 'This device does not offer files.'
        : viewOnly ? 'Files are off in view only.' : 'Files are off while this device cannot be controlled.';
      fNote.hidden = ok;
      fNav.hidden = fBody.hidden = bSend.hidden = !ok;
      if (ok && !fsData && !fsWait) list(fsPath);
    }
    function list(path) {
      if (!filesOk()) return;
      fsWait++;
      send({ t: 'fs', op: 'list', path });
      clearTimeout(fsT); // the newest request's answer is the one that counts
      fsT = setTimeout(() => { fsWait = 1; onList({ err: 'The device did not answer. Try again.' }); }, 10000);
      fMsg.textContent = 'Loading';
      fMsg.hidden = false;
      fList.replaceChildren();
    }
    function onList(m) {
      if (fsWait) fsWait--;
      if (fsWait) return; // a newer request is on its way
      clearTimeout(fsT);
      if (m.err != null) fsData = { err: String(m.err) }; // stay in the folder shown; its last crumb reloads it
      else {
        if (typeof m.path === 'string') fsPath = m.path;
        fsParent = typeof m.parent === 'string' ? m.parent : '';
        fsData = { entries: Array.isArray(m.entries) ? m.entries : [] };
      }
      const orphan = !document.activeElement || document.activeElement === document.body;
      const trail = [];
      if (fsPath.startsWith('/')) trail.push(['/', '/']);
      for (const x of fsPath.matchAll(/[^\\/]+/g)) {
        const p = fsPath.slice(0, x.index + x[0].length);
        trail.push([x[0], /^[a-z]:$/i.test(p) ? p + '\\' : p]);
      }
      crumbs.replaceChildren(...trail.map(([n, p], i) => {
        const c = h('button', { type: 'button', class: 'rv-crumb', text: n, title: p, onclick: () => list(p) });
        if (i === trail.length - 1) c.setAttribute('aria-current', 'location');
        return c;
      }));
      crumbs.scrollLeft = crumbs.scrollWidth;
      bUp.disabled = !fsParent || fsParent === fsPath;
      const ents = (fsData.entries || []).filter(e => e && typeof e.name === 'string' && e.name && e.name !== '.' && e.name !== '..')
        .slice(0, 2000).sort((a, b) => !!b.dir - !!a.dir);
      fMsg.textContent = fsData.err != null ? 'Could not open this folder. ' + fsData.err : ents.length ? '' : 'This folder is empty.';
      fMsg.hidden = !fMsg.textContent;
      fList.replaceChildren(...ents.map(entry));
      if (orphan && filesOpen) (fList.querySelector('button') || files).focus({ preventScroll: true }); // the clicked row is gone
    }
    function entry(e) {
      const size = num(e.size), when = num(e.mtime) ? new Date(e.mtime * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '';
      const meta = e.dir ? when : [size != null ? fmtB(size) : '', when].filter(Boolean).join(' · ');
      const li = h('li', { class: 'rv-fe' },
        h('button', { type: 'button', class: 'rv-fe-main', title: e.name, onclick: () => (e.dir ? list(join(fsPath, e.name)) : download(e)) },
          icon(e.dir ? 'folder' : 'file'), h('span', { class: 'rv-fe-name', text: e.name }), h('span', { class: 'rv-fe-meta', text: meta })));
      if (e.dir) li.dataset.dir = '1';
      else li.append(btn('Download ' + e.name, 'dl', () => download(e)));
      return li;
    }
    // Chromium saves straight to disk (picker opened inside the click); everything else collects a Blob, up to 1 GB.
    async function download(e) {
      if (!filesOk()) return;
      const size = num(e.size) || 0, path = join(fsPath, e.name);
      if (size > MAX_FILE) return xfer('down', e.name, size, OVER_4G);
      let w = null;
      if (window.showSaveFilePicker) {
        try { w = await (await window.showSaveFilePicker({ suggestedName: e.name })).createWritable(); }
        catch (err) { if (err && err.name === 'AbortError') return; } // picker dismissed; other failures fall back to a Blob
      }
      if (!filesOk()) { if (w) w.abort().catch(() => {}); return; }
      if (!w && size > BLOB_MAX) return xfer('down', e.name, size, TOO_BIG);
      const t = xfer('down', e.name, size);
      Object.assign(t, { path, w, chain: Promise.resolve(), parts: [] });
      pump();
    }
    function queueUp(list, dirs = []) {
      if (!filesOk()) return;
      for (const f of list) { const t = xfer('up', f.name, f.size, f.size > MAX_FILE ? OVER_4G : ''); if (busy(t)) t.file = f; }
      for (const d of dirs) xfer('up', d, 0, 'Folders cannot be sent. Send the files inside it instead.');
      toggleFiles(true);
      pump();
    }
    function xfer(dir, name, size, err) {
      const t = { id: ++xid, dir, name, size, got: 0, st: 'queued', msg: '', t0: 0 };
      t.bar = h('progress', { class: 'rv-xf-bar', max: '1', value: '0' });
      t.info = h('span', { class: 'rv-xf-info' });
      t.bx = h('button', { type: 'button', class: 'rv-btn rv-text rv-xf-b', onclick: () => (busy(t) ? end(t, 'cancel', 'Cancelled.', true) : dismiss(t)) });
      t.row = h('li', { class: 'rv-xf' }, h('div', { class: 'rv-xf-top' }, icon(dir === 'down' ? 'dl' : 'ul'), h('span', { class: 'rv-xf-name', text: name, title: name }), t.bx), t.bar, t.info);
      xfers.set(t.id, t);
      xList.prepend(t.row);
      if (err) end(t, 'err', err);
      draw();
      return t;
    }
    function end(t, st, msg, tell) {
      if (!busy(t)) return;
      if (tell && t.st === 'run') send({ t: 'file', op: 'cancel', id: t.id });
      t.st = st;
      t.msg = msg;
      if (t.w) { t.w.abort().catch(() => {}); t.w = null; }
      t.parts = t.file = null;
      if (t.wake) t.wake();
      pump();
    }
    function dismiss(t) {
      const f = t.row.contains(document.activeElement);
      t.row.remove();
      xfers.delete(t.id);
      draw();
      if (f) (xList.querySelector('.rv-xf-b') || files).focus({ preventScroll: true });
    }
    // at most 2 transfers at a time (the host's limit), the rest wait in order
    function pump() {
      if (filesOk()) {
        let n = 0;
        for (const t of xfers.values()) if (t.st === 'run') n++;
        for (const t of xfers.values()) {
          if (n >= 2) break;
          if (t.st !== 'queued') continue;
          n++;
          t.st = 'run';
          t.t0 = performance.now();
          if (t.dir === 'down') send({ t: 'file', op: 'get', id: t.id, path: t.path });
          else upload(t);
        }
      }
      draw();
    }
    function draw() {
      if (xfT || closed) return;
      xfT = requestAnimationFrame(() => {
        xfT = 0;
        const now = performance.now();
        let n = 0;
        for (const t of xfers.values()) {
          const on = busy(t), sec = (now - t.t0) / 1000;
          if (on) n++;
          t.row.dataset.st = t.st;
          t.bx.textContent = on ? 'Cancel' : 'Dismiss';
          t.bx.setAttribute('aria-label', (on ? 'Cancel ' : 'Dismiss ') + t.name);
          t.bar.hidden = !on;
          t.bar.value = t.size ? Math.min(1, t.got / t.size) : 0;
          t.info.textContent = t.st === 'queued' ? (t.dir === 'down' ? 'Download' : 'Upload') + ', waiting'
            : t.st === 'run' ? `${t.dir === 'down' ? 'Downloading' : 'Sending'}, ${fmtB(t.got)} of ${fmtB(t.size)}` + (sec > 0.5 && t.got ? `, ${fmtB(t.got / sec)}/s` : '')
            : t.msg;
        }
        xHead.hidden = xList.hidden = !xfers.size;
        bFiles.dataset.busy = n ? '1' : '';
        bFiles.setAttribute('aria-label', n ? `Files, ${n} transfer${n > 1 ? 's' : ''} running` : 'Files');
      });
    }
    function onFile(m) {
      const t = xfers.get(m.id);
      if (!t) return;
      if (m.op === 'cancel') return end(t, 'cancel', 'Cancelled on the device.');
      if (m.err != null) return end(t, 'err', String(m.err) || 'The device could not finish this transfer.');
      if (t.st !== 'run') return;
      if (m.done) return t.dir === 'down' ? downDone(t) : end(t, 'done', typeof m.path === 'string' && m.path ? 'Saved to ' + m.path : 'Saved on the device.');
      if (t.dir === 'up') { if (num(m.got) != null) { t.got = Math.max(t.got, m.got); if (t.wake) t.wake(); } }
      else if (num(m.size) != null) { t.size = m.size; if (!t.w && t.size > BLOB_MAX) end(t, 'err', TOO_BIG, true); }
      draw();
    }
    function fileChunk(b) {
      const t = xfers.get(new DataView(b.buffer, b.byteOffset + 1, 4).getUint32(0));
      if (!t || t.dir !== 'down' || t.st !== 'run') return;
      const d = b.subarray(5), ack = () => send({ t: 'file', op: 'ack', id: t.id, got: t.got });
      t.rx = (t.rx || 0) + d.length; // received, written or not: never more than the device announced
      if (t.rx > t.size) return end(t, 'err', 'The device sent more than the file size. The download was stopped.', true);
      if (t.w) { // ack only what reached the disk, so a slow disk slows the host instead of filling memory
        const w = t.w;
        t.chain = t.chain.then(() => t.st === 'run' && w.write(d).then(() => { t.got += d.length; ack(); draw(); }))
          .catch(() => end(t, 'err', 'Could not save the file. Check the free space on this computer.', true));
        return;
      }
      t.parts.push(d);
      t.got += d.length;
      if (t.got > BLOB_MAX) return end(t, 'err', TOO_BIG, true);
      ack();
      draw();
    }
    function downDone(t) {
      const short = 'The download stopped before the end. Try again.';
      if (t.w) {
        const w = t.w;
        t.chain.then(() => { if (t.st !== 'run') return; if (t.got !== t.size) throw 0; t.w = null; return w.close().then(() => end(t, 'done', 'Saved.')); })
          .catch(() => end(t, 'err', short));
        return;
      }
      if (t.got !== t.size) return end(t, 'err', short);
      const url = URL.createObjectURL(new Blob(t.parts, { type: 'application/octet-stream' }));
      const a = h('a', { href: url, download: t.name, hidden: '' });
      root.append(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 60000);
      end(t, 'done', 'Downloaded.');
    }
    // put, then 0x10 chunks, never more than 4 MB past the host's last "got", then end
    async function upload(t) {
      const s = ws;
      send({ t: 'file', op: 'put', id: t.id, name: t.name, size: t.size });
      try {
        for (let off = 0; off < t.size;) {
          if (t.st !== 'run' || s !== ws) return;
          const n = Math.min(CHUNK, t.size - off);
          if (off + n - t.got > AHEAD) { await new Promise(r => { t.wake = r; }); t.wake = null; continue; }
          const d = new Uint8Array(await t.file.slice(off, off + n).arrayBuffer());
          if (t.st !== 'run' || s !== ws || s.readyState !== 1) return;
          const m = new Uint8Array(5 + d.length);
          m[0] = 0x10;
          new DataView(m.buffer).setUint32(1, t.id);
          m.set(d, 5);
          s.send(m);
          off += n;
        }
        if (t.st === 'run' && s === ws) send({ t: 'file', op: 'end', id: t.id });
      } catch { end(t, 'err', 'Could not read this file on this computer.', true); }
    }
    // drop files on the picture or the panel; a dropped folder is refused in words
    const hasFiles = e => !!e.dataTransfer && [...e.dataTransfer.types].includes('Files');
    root.addEventListener('dragover', e => {
      if (!hasFiles(e)) return;
      e.preventDefault(); // never let the browser open the file instead
      e.dataTransfer.dropEffect = filesOk() ? 'copy' : 'none';
      root.classList.toggle('rv-drop', filesOk());
    });
    root.addEventListener('dragleave', e => { if (!root.contains(e.relatedTarget)) root.classList.remove('rv-drop'); });
    root.addEventListener('drop', e => {
      root.classList.remove('rv-drop');
      if (!hasFiles(e)) return;
      e.preventDefault();
      const fl = [], dirs = [];
      for (const it of e.dataTransfer.items) {
        if (it.kind !== 'file') continue;
        const en = it.webkitGetAsEntry && it.webkitGetAsEntry(), f = it.getAsFile();
        if (en && en.isDirectory) dirs.push(en.name);
        else if (f) fl.push(f);
      }
      queueUp(fl, dirs);
    });

    // ---- sound (cap "audio"): off until the user clicks (autoplay rules); s16le PCM, about 100 ms buffered ----
    function toggleAudio() {
      if (audioOn) return audioStop(true);
      if (!AC || !caps.has('audio')) return;
      if (!au) {
        let ctx;
        try { ctx = new AC({ latencyHint: 'interactive', sampleRate: 48000 }); } catch { ctx = new AC(); }
        const gain = ctx.createGain();
        gain.gain.value = vol;
        gain.connect(ctx.destination);
        au = { ctx, gain, rate: 0, ch: 0, at: 0, node: null, wl: null, loading: false };
      }
      au.ctx.resume().catch(() => {});
      audioOn = true;
      send({ t: 'audio', on: true });
      render();
    }
    function audioStop(tell) {
      if (tell) send({ t: 'audio', on: false });
      audioOn = false;
      if (au) { au.ctx.close().catch(() => {}); au = null; }
      render();
    }
    // The worklet needs a secure context and a CSP that allows blob: scripts; without either, scheduled buffers play it.
    function audioFormat(m) {
      const rate = num(m.rate), ch = num(m.ch) | 0, a = au;
      if (!a || !audioOn || !(rate >= 8000 && rate <= 192000) || ch < 1 || ch > 8) return;
      a.rate = rate;
      a.ch = ch;
      a.at = 0;
      if (a.node) { a.node.disconnect(); a.node = null; }
      if (rate !== a.ctx.sampleRate || !a.ctx.audioWorklet || a.wl === false) return; // buffers resample any rate
      if (!a.wl) {
        const u = URL.createObjectURL(new Blob([WORKLET], { type: 'text/javascript' }));
        a.wl = a.ctx.audioWorklet.addModule(u);
        a.wl.then(() => URL.revokeObjectURL(u), () => URL.revokeObjectURL(u));
      }
      a.loading = true;
      a.wl.then(() => {
        if (au !== a || a.rate !== rate || a.ch !== ch || a.node) return;
        a.node = new AudioWorkletNode(a.ctx, 'rv-pcm', { numberOfInputs: 0, outputChannelCount: [ch] });
        a.node.connect(a.gain);
      }).catch(() => { a.wl = false; }).then(() => { a.loading = false; });
    }
    function pcm(b) {
      const a = au;
      if (!a || !a.rate || !audioOn) return;
      const ch = a.ch, n = Math.floor((b.length - 1) / 2 / ch);
      if (!n) return;
      const dv = new DataView(b.buffer, b.byteOffset + 1, b.length - 1), out = [];
      for (let c = 0; c < ch; c++) out.push(new Float32Array(n));
      for (let i = 0, o = 0; i < n; i++) for (let c = 0; c < ch; c++, o += 2) out[c][i] = dv.getInt16(o, true) / 32768;
      if (a.node) return a.node.port.postMessage(out, out.map(x => x.buffer));
      if (a.loading) return;
      const now = a.ctx.currentTime;
      if (a.at < now) a.at = now + 0.1; // underrun: build up about 100 ms again
      else if (a.at - now > 0.3) return; // more than 300 ms queued: drop
      const buf = a.ctx.createBuffer(ch, n, a.rate);
      out.forEach((x, c) => buf.copyToChannel(x, c));
      const src = a.ctx.createBufferSource();
      src.buffer = buf;
      src.connect(a.gain);
      src.start(a.at);
      a.at += n / a.rate;
    }

    // ---- input helpers ----
    function flush() {
      raf = 0;
      if (mmPending) { send({ t: 'mm', x: r4(mmPending.x), y: r4(mmPending.y) }); mmPending = null; }
      const dx = Math.trunc(wheel.dx), dy = Math.trunc(wheel.dy);
      if (dx || dy) { send({ t: 'wh', dx, dy }); wheel.dx -= dx; wheel.dy -= dy; }
    }
    const sched = () => { if (!raf) raf = requestAnimationFrame(flush); };
    const press = b => { mmPending = null; buttons.add(b); send({ t: 'md', b, x: r4(cur.x), y: r4(cur.y) }); };
    const release = b => { if (buttons.delete(b)) send({ t: 'mu', b, x: r4(cur.x), y: r4(cur.y) }); };
    const click = b => { press(b); release(b); };
    function releaseAll() {
      for (const code of keys) send({ t: 'ku', code });
      keys.clear();
      for (const b of [...buttons]) release(b);
      mmPending = null;
    }
    function chord(...codes) {
      if (!can()) return;
      for (const code of codes) send({ t: 'kd', code });
      for (const code of codes.reverse()) send({ t: 'ku', code });
    }
    // typed text in chunks of at most 1000 characters; held modifiers are let go first so the host types plain text,
    // then pressed again so a still-held Ctrl keeps working. Line ends become \n (the host turns each \n into Enter).
    function sendText(s) {
      if (!can() || !s) return;
      s = s.replace(/\r\n?/g, '\n');
      const held = [...keys].filter(code => MODS.test(code));
      for (const code of held) send({ t: 'ku', code });
      for (const c of chunks(s, 1000)) send({ t: 'text', s: c });
      for (const code of held) send({ t: 'kd', code });
    }
    const takeSticky = () => {
      const m = [...sticky];
      sticky.clear();
      for (const b of mods) b.setAttribute('aria-pressed', 'false');
      return m;
    };
    const codeOf = ch => (/^[a-z]$/i.test(ch) ? 'Key' + ch.toUpperCase() : /^\d$/.test(ch) ? 'Digit' + ch : ch === ' ' ? 'Space' : '');
    // 8000 characters stay under the 64 KB message limit even fully escaped.
    function termSend(d) {
      if (viewOnly) return;
      for (const c of chunks(d, 8000)) send({ t: 'term', op: 'in', d: c });
    }
    function tapKey(code) {
      const m = takeSticky();
      if (kbTarget === 'term' && termOpen) {
        const d = TERM_KEYS[code];
        return d && termSend(d);
      }
      chord(...m, code);
    }
    function typeText(s) {
      if (kbTarget === 'term' && termOpen) {
        if (sticky.has('ControlLeft') && s.length === 1 && /[a-z@\[\\\]^_]/i.test(s)) { takeSticky(); return termSend(String.fromCharCode(s.toUpperCase().charCodeAt(0) - 64)); }
        takeSticky();
        return termSend(s);
      }
      const m = takeSticky();
      if (m.length && s.length === 1 && codeOf(s)) return chord(...m, codeOf(s));
      sendText(s);
    }
    const pos = e => {
      const r = canvas.getBoundingClientRect();
      return { x: clamp((e.clientX - r.left) / r.width), y: clamp((e.clientY - r.top) / r.height) };
    };
    const setCur = p => { cur = p; lastLocal = performance.now(); placeCursor(); };
    const mapKey = c => (cmdCtrl && c === 'MetaLeft' ? 'ControlLeft' : cmdCtrl && c === 'MetaRight' ? 'ControlRight' : c);

    // ---- mouse and keyboard ----
    canvas.addEventListener('pointerdown', e => {
      if (e.pointerType === 'touch') return touchDown(e);
      canvas.focus({ preventScroll: true });
      kbTarget = 'screen';
      e.preventDefault();
      const b = [0, 1, 2][e.button];
      if (b == null || !can()) return;
      canvas.setPointerCapture(e.pointerId);
      setCur(pos(e));
      press(b);
    });
    canvas.addEventListener('pointermove', e => {
      if (e.pointerType === 'touch') return touchMove(e);
      if (!fit) pan(e);
      if (!can()) return;
      cur = pos(e);
      lastLocal = performance.now();
      // a second button pressed or released while another is held arrives as pointermove with e.button set
      const b = [0, 1, 2][e.button];
      if (b != null) { if (e.buttons & [1, 4, 2][b]) { if (!buttons.has(b)) press(b); } else release(b); return; }
      mmPending = cur;
      sched();
    });
    canvas.addEventListener('pointerup', e => {
      if (e.pointerType === 'touch') return touchUp(e, false);
      const b = [0, 1, 2][e.button];
      if (b == null) return;
      cur = pos(e);
      release(b);
    });
    canvas.addEventListener('pointercancel', e => (e.pointerType === 'touch' ? touchUp(e, true) : releaseAll()));
    canvas.addEventListener('contextmenu', e => e.preventDefault());
    canvas.addEventListener('wheel', e => {
      e.preventDefault();
      if (!can()) return;
      const k = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 360 : 1.2; // 3 lines or 100 px per notch = 120
      wheel.dx += e.deltaX * k;
      wheel.dy += e.deltaY * k;
      sched();
    }, { passive: false });
    function paste() {
      vHeld = true;
      navigator.clipboard.readText().then(t => { if (!t) throw 0; sendText(t); }).catch(() => {
        if (!can()) return; // no clipboard access: send the key itself as before
        send({ t: 'kd', code: 'KeyV' });
        if (vHeld) keys.add('KeyV'); else send({ t: 'ku', code: 'KeyV' });
      });
    }
    canvas.addEventListener('keydown', e => {
      if (!can()) return;
      e.preventDefault();
      const code = mapKey(e.code);
      if (!code) return;
      if (e.repeat) { if (keys.has(code)) send({ t: 'kd', code }); return; } // hosts do not auto-repeat injected keys
      if (keys.has(code)) return;
      if (code === 'KeyV' && (e.ctrlKey || e.metaKey) && !e.altKey && navigator.clipboard && navigator.clipboard.readText) return paste();
      keys.add(code);
      send({ t: 'kd', code });
    });
    canvas.addEventListener('keyup', e => {
      if (e.code === 'KeyV') vHeld = false;
      if (!can()) return;
      e.preventDefault();
      const code = mapKey(e.code);
      if (keys.delete(code)) send({ t: 'ku', code });
      // macOS browsers fire no keyup for keys let go while Cmd is down: let them go with Cmd
      if (/^Meta/.test(e.code)) for (const c of [...keys]) if (!MODS.test(c)) { keys.delete(c); send({ t: 'ku', code: c }); }
    });
    canvas.addEventListener('blur', releaseAll);

    // ---- touch: trackpad (default) or direct ----
    const dist = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
    const mid = (a, b) => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });
    function touchDown(e) {
      e.preventDefault();
      canvas.setPointerCapture(e.pointerId);
      touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
      touched = true;
      kbTarget = 'screen';
      const now = performance.now();
      if (touches.size === 1) {
        g = { two: false, t0: now, x0: e.clientX, y0: e.clientY, moved: false, down: false,
          drag: now - lastTap.t < 300 && Math.hypot(e.clientX - lastTap.x, e.clientY - lastTap.y) < 40 };
        if (mode === 'direct') setCur(pos(e));
        else placeCursor();
      } else if (touches.size === 2) {
        if (g && g.down) release(0);
        const [a, b] = [...touches.values()];
        g = { two: true, t0: now, d0: dist(a, b), m: mid(a, b), m0: mid(a, b), z0: zoom, moved: false, pinch: false };
      }
    }
    function touchMove(e) {
      const t = touches.get(e.pointerId);
      if (!t || !g) return;
      const dx = e.clientX - t.x, dy = e.clientY - t.y;
      t.x = e.clientX;
      t.y = e.clientY;
      if (!g.two) {
        if (!g.moved && Math.hypot(e.clientX - g.x0, e.clientY - g.y0) > 8) {
          g.moved = true;
          if ((g.drag || mode === 'direct') && can()) { press(0); g.down = true; }
        }
        if (!g.moved) return;
        if (mode === 'direct') setCur(pos(e));
        else { const r = canvas.getBoundingClientRect(); setCur({ x: clamp(cur.x + dx * 1.5 / r.width), y: clamp(cur.y + dy * 1.5 / r.height) }); }
        if (can()) { mmPending = cur; sched(); }
      } else if (touches.size === 2) {
        const [a, b] = [...touches.values()], d = dist(a, b), m = mid(a, b);
        if (!g.moved && (Math.abs(d - g.d0) > 30 || dist(m, g.m0) > 10)) { g.moved = true; g.pinch = Math.abs(d - g.d0) > 30; }
        if (g.pinch) zoomTo(g.z0 * d / g.d0, m);
        else if (g.moved && mode === 'direct' && (zoom > 1 || !fit)) { stage.scrollLeft -= m.x - g.m.x; stage.scrollTop -= m.y - g.m.y; } // pan the zoomed picture
        else if (g.moved && can()) { wheel.dx -= (m.x - g.m.x) * 3; wheel.dy -= (m.y - g.m.y) * 3; sched(); }
        g.m = m;
      }
    }
    function touchUp(e, cancel) {
      touches.delete(e.pointerId);
      if (!g) return;
      const now = performance.now();
      if (!g.two) {
        if (g.down) release(0);
        else if (!cancel && !g.moved && now - g.t0 < 300 && can()) {
          if (mode === 'direct') setCur(pos(e));
          click(0);
          lastTap = { t: now, x: e.clientX, y: e.clientY };
        }
        g = null;
      } else if (!touches.size) {
        if (!cancel && !g.moved && now - g.t0 < 350 && can()) click(2);
        g = null;
      }
    }

    // ---- phone keyboard ----
    function flushKb() {
      const v = kbIn.value;
      if (!v) tapKey('Backspace');
      const s = v.split(SENTINEL).join('');
      if (s) typeText(s);
      kbIn.value = SENTINEL;
    }
    kbIn.addEventListener('input', e => { if (!e.isComposing) flushKb(); });
    kbIn.addEventListener('compositionend', flushKb);
    kbIn.addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); tapKey('Enter'); } });

    // ---- terminal ----
    const measure = document.createElement('canvas').getContext('2d');
    function termSize() {
      const cs = getComputedStyle(termOut);
      measure.font = cs.font;
      const cw = measure.measureText('MMMMMMMMMM').width / 10 || 8, lh = parseFloat(cs.lineHeight) || 18;
      const w = termOut.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
      const ht = termOut.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom);
      return { cols: Math.max(20, Math.floor(w / cw)), rows: Math.max(5, Math.floor(ht / lh)) };
    }
    function termResize() {
      if (!termOpen) return;
      clearTimeout(resizeT);
      resizeT = setTimeout(() => {
        const { cols, rows } = termSize(), k = cols + 'x' + rows;
        if (termOpen && k !== termDims) { termDims = k; send({ t: 'term', op: 'resize', cols, rows }); }
      }, 150);
    }
    function toggleTerm(on = !termOpen) {
      if (on && (!(hello && hello.terminal) || viewOnly)) return;
      if (on === termOpen) return;
      termOpen = on;
      term.hidden = !on;
      bTerm.setAttribute('aria-pressed', String(on));
      root.classList.toggle('rv-term-on', on);
      if (on) {
        render();
        const { cols, rows } = termSize();
        termDims = cols + 'x' + rows;
        send({ t: 'term', op: 'open', cols, rows });
        kbTarget = 'term';
        termOut.focus({ preventScroll: true });
      } else {
        send({ t: 'term', op: 'close' });
        kbTarget = 'screen';
        render();
      }
      layout();
    }
    // Line model: finished lines in termHead, the last line in termLine with a cursor column, so \b, \r and the
    // cursor/erase CSI readline uses (C, D, G, K, P, @, 2J) edit the line like a terminal would.
    // ponytail: one line only, no rows or scroll regions; full-screen apps (vim, top) still render poorly.
    function termWrite(bytes) {
      let head = termHead, L = termLine, c = termCol;
      for (const m of termDec.decode(bytes, { stream: true }).matchAll(TERM_RE)) {
        if (m[4]) {
          const s = m[4];
          if (c > L.length) L = L.padEnd(c);
          L = c === L.length ? L + s : L.slice(0, c) + s + L.slice(c + s.length);
          c += s.length;
        } else if (m[3]) {
          const ch = m[3];
          if (ch === '\n') { head += L + '\n'; L = ''; c = 0; }
          else if (ch === '\r') c = 0;
          else if (ch === '\b') c = Math.max(0, c - 1);
          else c = (c >> 3) * 8 + 8;
        } else if (m[2] && m[1][0] !== '?') {
          const p = parseInt(m[1], 10), n = p || 1, k = p || 0;
          switch (m[2]) {
            case 'C': c += n; break;
            case 'D': c = Math.max(0, c - n); break;
            case 'G': c = n - 1; break;
            case 'K': L = k === 2 ? '' : k === 1 ? ''.padEnd(c + 1) + L.slice(c + 1) : L.slice(0, c); break;
            case 'P': L = L.slice(0, c) + L.slice(c + n); break;
            case '@': L = L.slice(0, c) + ''.padEnd(n) + L.slice(c); break;
            case 'J': if (k >= 2) { head = ''; L = ''; c = 0; } break;
            case 'H': c = 0; break;
          }
        }
      }
      if (head.length > 200000) head = head.slice(-150000);
      termHead = head;
      termLine = L;
      termCol = c;
      if (!termDirty) {
        termDirty = true;
        requestAnimationFrame(() => {
          termDirty = false;
          const atEnd = termOut.scrollTop + termOut.clientHeight >= termOut.scrollHeight - 4;
          termOut.textContent = termHead + termLine;
          if (atEnd) termOut.scrollTop = termOut.scrollHeight;
        });
      }
    }
    termOut.addEventListener('keydown', e => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'v') return; // paste event handles it
      let d = TERM_KEYS[e.key];
      if (!d && e.ctrlKey && !e.altKey && !e.metaKey && e.key.length === 1) {
        const c = e.key.toUpperCase().charCodeAt(0);
        if (c >= 64 && c < 96) d = String.fromCharCode(c - 64);
      }
      if (!d && e.key.length === 1 && !e.metaKey && (!e.ctrlKey || e.altKey)) d = e.key;
      if (!d) return;
      e.preventDefault();
      termSend(d);
    });
    termOut.addEventListener('paste', e => { e.preventDefault(); termSend(e.clipboardData.getData('text')); });
    termOut.addEventListener('focus', () => { kbTarget = 'term'; });

    // ---- toolbar actions ----
    function toggleView() {
      viewOnly = !viewOnly;
      if (viewOnly) { releaseAll(); for (const t of xfers.values()) end(t, 'cancel', 'Stopped. View only is on.', true); }
      bView.setAttribute('aria-pressed', String(viewOnly));
      label(bView, viewOnly ? 'View only (on)' : 'View only');
      send({ t: 'view', on: viewOnly });
      render();
    }
    const fsEl = () => document.fullscreenElement || document.webkitFullscreenElement;
    function toggleFull() {
      if (fsEl()) (document.exitFullscreen || document.webkitExitFullscreen).call(document);
      else {
        const f = root.requestFullscreen || root.webkitRequestFullscreen;
        if (f) Promise.resolve(f.call(root)).catch(() => {});
      }
    }
    function onFs() {
      const on = fsEl() === root;
      root.classList.toggle('rv-fs', on);
      setIcon(bFull, on ? 'unfull' : 'full');
      label(bFull, on ? 'Exit fullscreen' : 'Fullscreen');
      const kb = navigator.keyboard;
      if (kb && kb.lock) on ? kb.lock().catch(() => {}) : kb.unlock();
      if (on) armHide(); else showBar();
      layout();
    }
    function onVis() {
      if (document.hidden) { releaseAll(); send({ t: 'stop' }); }
      else if (live()) start();
    }
    function disconnect() {
      const code = (bye && bye.code) || undefined;
      close();
      if (onClose) onClose(code);
    }
    document.addEventListener('fullscreenchange', onFs);
    document.addEventListener('webkitfullscreenchange', onFs);
    document.addEventListener('visibilitychange', onVis);
    window.addEventListener('blur', releaseAll);

    function close() {
      if (closed) return;
      releaseAll();
      if (termOpen) send({ t: 'term', op: 'close' });
      closed = true;
      for (const t of xfers.values()) end(t, 'cancel', 'Closed.', true);
      if (au) { au.ctx.close().catch(() => {}); au = null; }
      cancelAnimationFrame(xfT);
      clearTimeout(retryT);
      clearTimeout(hideT);
      clearTimeout(resizeT);
      clearInterval(pingT);
      cancelAnimationFrame(raf);
      cancelAnimationFrame(paintT);
      if (next) next.close();
      next = pend = null;
      vreset();
      ro.disconnect();
      document.removeEventListener('fullscreenchange', onFs);
      document.removeEventListener('webkitfullscreenchange', onFs);
      document.removeEventListener('visibilitychange', onVis);
      window.removeEventListener('blur', releaseAll);
      if (fsEl() === root) toggleFull();
      if (ws) { ws.onclose = ws.onmessage = null; try { ws.close(1000); } catch {} }
      root.remove();
    }

    connect();
    return { close };
  }

  window.RemoteViewer = { open };
})();
