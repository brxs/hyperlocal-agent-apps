"""Two-process restaurant/guest MCP demo. Uses real mDNS discovery and peer RPC.

Run from any directory: python3 /path/to/examples/pizzeria/demo.py
No order is placed until you enter 'order'. All orders are simulated.
"""
import argparse
import hashlib
import json
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / 'target/debug/hyperlocal-agent-apps'
COMPONENT = ROOT / 'fixtures/pizzeria.wasm'
SERVICE = 'restaurant:pizzeria/service@0.1.0'


class Mcp:
    def __init__(self, *arguments):
        self.process = subprocess.Popen([str(BINARY), *arguments], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, text=True, cwd=ROOT)
        self.sequence = 0
        try:
            self.request('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {},
                                       'clientInfo': {'name': 'pizzeria-demo', 'version': '1'}})
            self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
        except BaseException:
            self.close()
            raise

    def send(self, value):
        self.process.stdin.write(json.dumps(value) + '\n')
        self.process.stdin.flush()

    def request(self, method, parameters):
        self.sequence += 1
        self.send({'jsonrpc': '2.0', 'id': self.sequence, 'method': method, 'params': parameters})
        while True:
            if not select.select([self.process.stdout], [], [], 30)[0]:
                raise TimeoutError(f'MCP {method} timed out; do not automatically retry mutations')
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError('MCP server exited')
            response = json.loads(line)
            if response.get('id') != self.sequence:
                continue
            if 'error' in response:
                raise RuntimeError(response['error'])
            return response['result']

    def tool(self, name, arguments):
        result = self.request('tools/call', {'name': name, 'arguments': arguments})
        if result.get('isError'):
            raise RuntimeError(result['content'][0]['text'])
        return json.loads(result['content'][0]['text'])

    def close(self):
        if self.process.poll() is not None:
            return
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pizza', default='hawaii', choices=['hawaii', 'berghain', 'checkpoint-gpt', 'kreuzberg-context', 'token-currywurst', 'merge-conflict', 'kitkat', 'about-blank'])
    parser.add_argument('--table', default=7, type=int, choices=range(1, 41))
    options = parser.parse_args()
    if not BINARY.is_file() or not COMPONENT.is_file():
        parser.error('Build the server with cargo build --locked; rebuild the component with examples/pizzeria/build.py if needed.')
    with socket.socket() as reservation:
        reservation.bind(('0.0.0.0', 0))
        port = reservation.getsockname()[1]
    request_id = str(uuid.uuid4())
    expected_digest = hashlib.sha256(COMPONENT.read_bytes()).hexdigest()
    with tempfile.TemporaryDirectory(prefix='hyperlocal-pizzeria-') as cache:
        restaurant = Mcp('--share-live', '--name', 'Forno Pizza', '--port', str(port),
                         '--plugin', f'restaurant={COMPONENT}')
        guest = None
        try:
            guest = Mcp('--download-dir', cache)
            discovery = guest.tool('discover_peers', {'wait_ms': 3000})
            peer = next((p for p in discovery['peers'] if p['name'] == 'Forno Pizza'
                         and any(url.endswith(f':{port}/') for url in p['urls'])), None)
            if peer:
                peer_url = next(url for url in peer['urls'] if url.endswith(f':{port}/'))
                via = 'mDNS'
            else:
                # This launcher owns the publishing process; local-only fallback is safe
                # when multicast is blocked. Real guests must use the restaurant's LAN IP.
                peer_url = f'http://127.0.0.1:{port}/'
                via = 'loopback fallback (multicast unavailable)'
            identity = {'peer_url': peer_url, 'plugin_id': 'restaurant'}
            info = guest.tool('inspect_peer_plugin', identity)
            if info['sha256'] != expected_digest:
                raise RuntimeError('Discovered peer does not match the simulated restaurant component')

            def call(name, arguments):
                result = guest.tool('call_peer_plugin', {**identity, 'sha256': info['sha256'],
                                                        'function': [SERVICE, name], 'arguments': arguments})
                return result['result']['results'][0]

            menu = call('get-menu', {})
            selected = next(p for p in menu['pizzas'] if p['id'] == options.pizza)
            view = guest.tool('open_peer_web', identity)
            print(json.dumps({'peer_name': info['peer_name'], 'discovery': via, 'peer_url': peer_url,
                              'menu_url': view['web_url'] + f'?table={options.table}&pizza={options.pizza}', 'pizza': selected['name'], 'table': options.table,
                              'note': 'Simulated restaurant. No payment or real kitchen dispatch.'}), flush=True)
            print('Commands: menu, order, orders, stop. Repeating order reuses the same request ID.', flush=True)
            while True:
                try:
                    command = input('forno> ').strip().lower()
                except EOFError:
                    break
                if command in ('stop', 'quit', 'exit'):
                    break
                if command == 'menu':
                    result = call('get-menu', {})
                elif command == 'order':
                    result = call('place-order', {'pizza-id': options.pizza, 'quantity': 1,
                                                 'table': options.table, 'request-id': request_id})
                elif command == 'orders':
                    result = call('get-table-orders', {'table': options.table})
                elif not command:
                    continue
                else:
                    print('Use menu, order, orders, or stop.', flush=True)
                    continue
                print(json.dumps({'peer_name': info['peer_name'], 'result': result}, indent=2), flush=True)
        finally:
            if guest:
                guest.close()
            restaurant.close()


if __name__ == '__main__':
    try:
        main()
    except KeyboardInterrupt:
        pass
