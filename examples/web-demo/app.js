const counter = document.querySelector('#counter');
const status = document.querySelector('#status');
const buttons = [...document.querySelectorAll('button')];
let busy = false;
async function request(path, options) {
  const response = await fetch(path, { cache: 'no-store', ...options });
  if (!response.ok) throw new Error(`HTTP ${response.status}. Reconnect the plugin if it has faulted.`);
  return response.json();
}
async function refresh() {
  if (busy) return;
  busy = true;
  try {
    counter.textContent = String(await request('/api/counter'));
    status.textContent = 'Connected · Agent changes appear here automatically';
  } catch (error) {
    status.textContent = error.message;
  } finally {
    busy = false;
  }
}
async function update(path) {
  if (busy) return;
  busy = true;
  buttons.forEach(button => { button.disabled = true; });
  try {
    counter.textContent = String(await request(path, { method: 'POST' }));
    status.textContent = 'Updated from the browser · The agent sees this value too';
  } catch (error) {
    status.textContent = error.message;
  } finally {
    buttons.forEach(button => { button.disabled = false; });
    busy = false;
  }
}
document.querySelector('#increment').addEventListener('click', () => update('/api/increment'));
document.querySelector('#reset').addEventListener('click', () => update('/api/reset'));
refresh();
// Polling keeps the demo small; no SSE or WebSocket support is needed.
setInterval(refresh, 1000);
document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });

// A remote web proxy provides the advertised peer label without modifying plugin HTML.
fetch('/.hyperlocal/connection', { cache: 'no-store' })
  .then(response => response.ok ? response.json() : null)
  .then(peer => {
    if (!peer) return;
    document.querySelector('#peer').textContent = `${peer.peer_name} / ${peer.plugin_id}`;
    document.title = `${peer.peer_name} · Shared WASM counter`;
  })
  .catch(() => {});
