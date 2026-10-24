"""Build four distinct self-contained vendor components from one guest crate."""
import os
from pathlib import Path
import shutil
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
VENDORS = ('tacos', 'ramen', 'spritz', 'taproom')
shutil.copyfile(ROOT / 'wit/web.wit', HERE / 'plugin/wit/deps/web/web.wit')
for vendor in VENDORS:
    environment = {**os.environ, 'FOODCOURT_VENDOR': vendor}
    subprocess.run(['cargo', 'build', '--locked', '--release', '--target', 'wasm32-unknown-unknown', '--manifest-path', str(HERE / 'plugin/Cargo.toml')], env=environment, check=True)
    target = ROOT / f'fixtures/foodcourt-{vendor}.wasm'
    subprocess.run(['cargo', 'run', '--locked', '--example', 'package_component', '--', str(HERE / 'plugin/target/wasm32-unknown-unknown/release/foodcourt_vendor_plugin.wasm'), str(target)], cwd=ROOT, check=True)
    print(target)
