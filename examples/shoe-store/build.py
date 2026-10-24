"""Build the Rust shoe store guest and package a self-contained WASM component."""
from pathlib import Path
import shutil
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
shutil.copyfile(ROOT / 'wit/web.wit', HERE / 'plugin/wit/deps/web/web.wit')
subprocess.run(['cargo', 'build', '--locked', '--release', '--target', 'wasm32-unknown-unknown', '--manifest-path', str(HERE / 'plugin/Cargo.toml')], check=True)
subprocess.run(['cargo', 'run', '--locked', '--example', 'package_component', '--', str(HERE / 'plugin/target/wasm32-unknown-unknown/release/shoe_store_plugin.wasm'), str(ROOT / 'fixtures/shoe-store.wasm')], cwd=ROOT, check=True)
print(ROOT / 'fixtures/shoe-store.wasm')
