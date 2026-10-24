"""Start four independent food-court peers. No orders are placed automatically."""
from contextlib import ExitStack
import json
from pathlib import Path
import sys
from peers import VENDORS

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from examples.pizzeria.demo import Mcp


def main():
    with ExitStack() as stack:
        for vendor, (name, port) in VENDORS.items():
            component = ROOT / 'fixtures' / f'foodcourt-{vendor}.wasm'
            publisher = Mcp('--share-live', '--name', name, '--port', str(port),
                            '--plugin', f'vendor={component}')
            stack.callback(publisher.close)
            info = publisher.tool('list_plugins', {})['plugins'][0]
            print(json.dumps({'peer_name': name, 'port': port, 'menu_url': info['web_url'],
                              'back_office_url': info['web_url'] + 'back-office'}), flush=True)
        print('Four independent peers are running. Discover them from your guest agent.\nAll orders are simulated. Enter stop or press Ctrl-C to close this demo.', flush=True)
        while True:
            try:
                command = input('foodcourt> ').strip().lower()
            except EOFError:
                break
            if command in ('stop', 'quit', 'exit'):
                break
            print('Use your guest agent to browse/order, or enter stop here.', flush=True)


if __name__ == '__main__':
    try:
        main()
    except KeyboardInterrupt:
        pass
