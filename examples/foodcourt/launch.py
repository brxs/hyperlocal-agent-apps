"""Launch one food-court vendor MCP server over stdio."""
import os
from pathlib import Path
import sys
from peers import VENDORS

ROOT = Path(__file__).resolve().parents[2]
if len(sys.argv) < 2 or sys.argv[1] not in VENDORS:
    sys.exit('Choose tacos, ramen, spritz, or taproom')
vendor = sys.argv[1]
name, port = VENDORS[vendor]
os.execv(str(ROOT / 'target/debug/hyperlocal-agent-apps'), [
    'hyperlocal-agent-apps', '--share-live', '--name', name, '--port', str(port),
    '--plugin', f'vendor={ROOT / "fixtures" / f"foodcourt-{vendor}.wasm"}',
    *sys.argv[2:],
])
