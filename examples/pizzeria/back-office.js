const ordersNode = document.querySelector('#office-orders');
const statusNode = document.querySelector('#connection-status');
const money = cents => new Intl.NumberFormat('en-IE', { style: 'currency', currency: 'EUR' }).format(cents / 100);
let polling = false;
let previous = '';
function element(tag, className, text) {
  const node = document.createElement(tag);
  node.className = className;
  node.textContent = text;
  return node;
}
async function refresh() {
  if (polling || document.hidden) return;
  polling = true;
  try {
    const response = await fetch('/api/open-orders', { cache: 'no-store' });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const { orders } = await response.json();
    statusNode.textContent = 'LIVE · UPDATES EVERY 1.5S';
    document.querySelector('#order-count').textContent = orders.length;
    const tables = new Set(orders.map(order => order.table)).size;
    document.querySelector('#office-summary').textContent = `${orders.length} open orders · ${tables} tables · ${money(orders.reduce((sum, order) => sum + order.total_cents, 0))} demo total`;
    const encoded = JSON.stringify(orders);
    if (previous !== encoded) {
      previous = encoded;
      ordersNode.replaceChildren();
      if (!orders.length) ordersNode.append(element('p', 'empty-receipt', 'No open orders. Orders from the menu or an agent will appear here.'));
      for (const order of [...orders].reverse()) {
        const card = element('article', 'receipt', '');
        const header = element('div', 'receipt-header', '');
        header.append(element('span', '', `ORDER #${String(order.id).padStart(3, '0')}`), element('span', 'received', 'RECEIVED ✓'));
        card.append(header, element('h3', 'office-table', `Table ${String(order.table).padStart(2, '0')}`), element('p', 'receipt-title', `${order.quantity} × ${order.pizza_name}`), element('p', 'receipt-total', money(order.total_cents)));
        ordersNode.append(card);
      }
    }
  } catch (error) { statusNode.textContent = 'OFFLINE · RETRYING'; }
  finally { polling = false; }
}
refresh();
setInterval(refresh, 1500);
document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
