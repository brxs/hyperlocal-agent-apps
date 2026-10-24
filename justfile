set positional-arguments

# List available recipes.
default:
    @just --list

# Launch the guest HTTP MCP endpoint for ChatGPT via Secure MCP Tunnel.
chatgpt *args:
    @cargo run --quiet --locked -- --http 127.0.0.1:47832 --download-dir .hyperlocal/guest-plugins "$@"

# Launch an MCP stdio server: restaurant, shoes, or guest. Extra arguments go to the server.
mcp role *args:
    @case "$1" in restaurant) shift; exec cargo run --quiet --locked -- --share-live --name "Forno Pizza" --port 47831 --plugin restaurant=fixtures/pizzeria.wasm "$@" ;; shoes) shift; exec cargo run --quiet --locked -- --share-live --name "Kiez Kicks" --port 47833 --plugin shoes=fixtures/shoe-store.wasm "$@" ;; guest) shift; exec cargo run --quiet --locked -- --download-dir .hyperlocal/guest-plugins "$@" ;; *) echo "Usage: just mcp restaurant|shoes|guest [server arguments...]" >&2; exit 2 ;; esac

# Open a connected Codex CLI session: restaurant, shoes, or guest. Extra arguments go to Codex.
run role *args:
    @case "$1" in restaurant|shoes|guest) ;; *) echo "Usage: just run restaurant|shoes|guest [Codex arguments...]" >&2; exit 2 ;; esac
    @cargo build --quiet --locked
    @exec python3 examples/pizzeria/codex.py "$@"

# Run the interactive restaurant + guest demo (optional: --pizza berghain --table 12).
demo *args:
    @cargo build --quiet --locked
    @python3 examples/pizzeria/demo.py "$@"
