const menuNode = document.querySelector('#menu');
const ordersNode = document.querySelector('#orders');
const tableInput = document.querySelector('#table');
const quantityInput = document.querySelector('#quantity');
const copyButton = document.querySelector('#copy-request');
const orderButton = document.querySelector('#place-order');
const orderStatus = document.querySelector('#order-status');
let submitting = false;
let ordered = false;
let items = [];
const parameters = new URLSearchParams(location.search);
const initialTable = Number(parameters.get('table'));
if (Number.isInteger(initialTable) && initialTable >= 1 && initialTable <= 40) tableInput.value = initialTable;
let selected = parameters.get('item') || '';
let peerName = 'Food Court';
let requestId = crypto.randomUUID();
let polling = false;
let lastOrders = '';
const money = cents => new Intl.NumberFormat('en-IE', { style: 'currency', currency: 'EUR' }).format(cents / 100);
const table = () => Number(tableInput.value);
const validTable = () => Number.isInteger(table()) && table() >= 1 && table() <= 40;
function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
async function api(path) {
  const response = await fetch(path, { cache: 'no-store' });
  if (!response.ok) throw new Error(`Restaurant unavailable (HTTP ${response.status})`);
  return response.json();
}
function updateSelection() {
  const item = items.find(p => p.id === selected);
  if (!item) return;
  document.querySelector('#selected-name').textContent = `${quantityInput.value} × ${item.name}`;
  document.querySelector('#selected-price').textContent = money(item.price_cents * Number(quantityInput.value));
  document.querySelector('#table-summary').textContent = String(table()).padStart(2, '0');
  document.querySelector('#order-heading').textContent = `${item.name}, coming right up?`;
  document.querySelector('#order-prompt').textContent = `At ${peerName}, order ${quantityInput.value} ${item.name} for table ${table()}.`;
  copyButton.disabled = !validTable() || submitting;
  orderButton.disabled = !validTable() || submitting;
  orderButton.textContent = submitting ? 'Sending order…' : ordered ? 'Place another demo order' : 'Place demo order';
  tableInput.disabled = submitting;
  quantityInput.disabled = submitting;
  for (const card of menuNode.querySelectorAll('.item-card')) {
    const active = card.dataset.item === selected;
    card.classList.toggle('selected', active);
    const button = card.querySelector('button');
    button.disabled = submitting;
    button.textContent = active ? 'Your pick ✓' : 'Choose ↗';
    button.setAttribute('aria-pressed', String(active));
  }
}
function renderMenu() {
  menuNode.replaceChildren();
  for (const item of items) {
    const card = element('article', 'item-card');
    card.dataset.item = item.id;
    const top = element('div', 'card-top');
    top.append(element('h3', '', item.name), element('span', 'price', money(item.price_cents)));
    const bottom = element('div', 'card-bottom');
    const tag = item.vegetarian ? 'Vegetarian' : 'House favourite';
    const choose = element('button', 'choose', 'Choose ↗');
    choose.type = 'button';
    choose.setAttribute('aria-label', `Choose ${item.name}`);
    choose.addEventListener('click', () => { selected = item.id; resetOrder(); updateSelection(); });
    bottom.append(element('span', 'tag', tag), choose);
    card.append(top, element('p', '', item.description), bottom);
    menuNode.append(card);
  }
  updateSelection();
}
function renderOrders(orders) {
  const encoded = JSON.stringify(orders);
  if (lastOrders === encoded) return;
  lastOrders = encoded;
  ordersNode.replaceChildren();
  if (!orders.length) {
    const empty = element('div', 'empty-receipt', 'No orders yet.');
    empty.append(document.createElement('br'), element('small', '', 'Your order will appear here.'));
    ordersNode.append(empty);
    return;
  }
  for (const order of [...orders].reverse()) {
    const receipt = element('article', 'receipt');
    const header = element('div', 'receipt-header');
    header.append(element('span', '', `ORDER #${String(order.id).padStart(3, '0')} · TABLE ${String(order.table).padStart(2, '0')}`), element('span', 'received', 'RECEIVED ✓'));
    const total = element('div', 'receipt-total');
    total.append(element('span', '', 'Total'), element('strong', '', money(order.total_cents)));
    receipt.append(header, element('p', 'receipt-title', `${order.quantity} × ${order.item_name}`), element('p', 'receipt-note', 'Received by the vendor peer. The agent and this menu see the same order.'), total, element('p', 'receipt-note', 'Demo receipt · no payment taken'));
    ordersNode.append(receipt);
  }
}
async function pollOrders() {
  if (polling || !validTable() || document.hidden) return;
  polling = true;
  const selectedTable = table();
  try {
    const data = await api(`/api/orders?table=${selectedTable}`);
    if (table() === selectedTable) renderOrders(data.orders);
    document.querySelector('#connection-status').textContent = 'LIVE';
  } catch (error) {
    document.querySelector('#connection-status').textContent = 'OFFLINE';
  } finally { polling = false; }
}
function resetOrder() {
  requestId = crypto.randomUUID(); ordered = false; orderStatus.textContent = 'No payment is taken.';
}
orderButton.addEventListener('click', async () => {
  if (submitting || !validTable() || !items.length) return;
  if (ordered) resetOrder();
  const input = { item_id: selected, quantity: Number(quantityInput.value), table: table(), request_id: requestId };
  submitting = true; updateSelection();
  orderStatus.textContent = 'Sending your demo order…';
  try {
    const response = await fetch('/api/orders', {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(input),
    });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || `HTTP ${response.status}`);
    ordered = true;
    orderStatus.textContent = `Order #${String(data.order.id).padStart(3, '0')} received. No payment taken.`;
    await pollOrders();
  } catch (error) {
    orderStatus.textContent = `Could not confirm order: ${error.message}. Retry to check the same request safely.`;
  } finally { submitting = false; updateSelection(); }
});
for (const control of [tableInput, quantityInput]) control.addEventListener('input', () => {
  resetOrder(); updateSelection(); lastOrders = ''; ordersNode.replaceChildren(); pollOrders();
});
copyButton.addEventListener('click', async () => {
  const item = items.find(p => p.id === selected);
  const text = `Find the live peer named "${peerName}". Inspect its foodcourt:vendor/service@0.1.0 interface, then call place-order with item-id="${item.id}", quantity=${quantityInput.value}, table=${table()}, request-id="${requestId}". This is a simulated order. Reuse this request ID for retries. Do not download or reconnect the vendor plugin.`;
  try { await navigator.clipboard.writeText(text); copyButton.textContent = 'Copied ✓'; }
  catch { copyButton.textContent = 'Copy unavailable — use the request above'; }
  setTimeout(() => { copyButton.replaceChildren(document.createTextNode('Copy order request ↗')); }, 2500);
});
(async () => {
  try {
    const data = await api('/api/menu');
    items = data.items;
    if (!items.some(p => p.id === selected)) selected = items[0].id;
    peerName = data.restaurant; document.querySelector('#brand').textContent = peerName; document.querySelector('#hero-title').textContent = data.tagline; document.querySelector('#category').textContent = data.category; document.documentElement.style.setProperty('--wine', data.accent); document.title = `${peerName} · Menu`; renderMenu();
    document.querySelector('#peer-name').textContent = peerName;
    try {
      const response = await fetch('/.hyperlocal/connection', { cache: 'no-store' });
      if (response.ok) {
        const context = await response.json();
        peerName = context.peer_name;
        document.querySelector('#peer-name').textContent = peerName;
        document.title = `${peerName} · Menu`;
        updateSelection();
      }
    } catch {}
    await pollOrders();
  } catch (error) { menuNode.replaceChildren(element('p', '', error.message)); copyButton.disabled = true; orderButton.disabled = true; }
})();
setInterval(pollOrders, 1500);
document.addEventListener('visibilitychange', () => { if (!document.hidden) pollOrders(); });
