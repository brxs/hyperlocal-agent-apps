"""Launch Codex with a session-local business or guest MCP configuration."""

import json
import os
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from examples.foodcourt.peers import VENDORS


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ('restaurant', 'shoes', 'guest', *VENDORS):
        sys.exit('Usage: codex.py restaurant|shoes|guest|tacos|ramen|spritz|taproom [Codex arguments...]')
    role = sys.argv[1]
    codex = shutil.which('codex')
    if codex is None:
        sys.exit('Codex CLI is required: install it and ensure codex is on PATH.')
    root = Path(__file__).resolve().parents[2]
    server_arguments = ['--download-dir', str(root / f'.hyperlocal/{role}-plugins')]
    if role == 'restaurant':
        server_arguments.extend([
            '--share-live', '--name', 'Forno Pizza', '--port', '47831',
            '--plugin', f'restaurant={root / "fixtures/pizzeria.wasm"}',
        ])
    elif role == 'shoes':
        server_arguments.extend([
            '--share-live', '--name', 'Kiez Kicks', '--port', '47833',
            '--plugin', f'shoes={root / "fixtures/shoe-store.wasm"}',
        ])
    elif role in VENDORS:
        name, port = VENDORS[role]
        server_arguments.extend([
            '--share-live', '--name', name, '--port', str(port),
            '--plugin', f'vendor={root / "fixtures" / f"foodcourt-{role}.wasm"}',
        ])
    configuration = {
        'command': str(root / 'target/debug/hyperlocal-agent-apps'),
        'args': server_arguments,
        'cwd': str(root),
        'enabled': True,
        'startup_timeout_sec': 30,
    }
    arguments = [codex]
    for key, value in configuration.items():
        arguments.extend(['-c', f'mcp_servers.hyperlocal_{role}.{key}={json.dumps(value)}'])
    # Keep the user's Codex settings and approval policy. No prompt or order is sent.
    os.execv(codex, [*arguments, *sys.argv[2:]])


if __name__ == '__main__':
    main()
