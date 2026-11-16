// Minimal reference client for the RMI control server. Testing only; see docs/AUTOMATION.md.
//
// Usage (Node >= 22, no dependencies):
//   node docs/automation-client.mjs <token> <account> [port=7810]
// Then open the Automation page in RMI, pick <account>, and click "Say hi".
// The token is in Settings > Automation. It stays the same across restarts until you regenerate it.

const [token, account, port = "7810"] = process.argv.slice(2);
if (!token || !account) {
  console.error("usage: node automation-client.mjs <token> <account> [port]");
  process.exit(2);
}

const url = `ws://127.0.0.1:${port}/control?token=${encodeURIComponent(token)}&account=${encodeURIComponent(account)}`;
const ws = new WebSocket(url);
const send = (t, data = {}, id) => ws.send(JSON.stringify({ t, id, data }));

ws.onopen = () => {
  console.log("connected");
  send("hello", { account });
  send("log", { text: "reference client connected", level: "info" });
  send("ui.create", { kind: "button", name: "Say hi" });
};

ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.t === "ui.event") {
    console.log("ui.event", m.data);
    send("log", { text: `clicked ${m.data.name}` });
  } else if (m.t === "message") {
    console.log("message", m.data);
  }
};

ws.onclose = (e) => {
  console.log(e.code === 4401 ? "rejected: wrong token" : `closed (${e.code})`);
  process.exit(e.code === 4401 ? 1 : 0);
};
ws.onerror = () => console.error("connection error (is RMI running with automation started?)");

// Keep-alive: client pings, RMI answers pong.
setInterval(() => {
  if (ws.readyState === WebSocket.OPEN) send("ping", {}, String(Date.now()));
}, 15000);
