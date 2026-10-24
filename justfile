set positional-arguments

# List available recipes.
default:
    @just --list

# Launch the guest HTTP MCP endpoint for ChatGPT via Secure MCP Tunnel.
chatgpt *args:
    @cargo run --quiet --locked -- --http 127.0.0.1:47832 --download-dir .hyperlocal/guest-plugins "$@"

# Launch a business or guest MCP stdio server. Food-court roles: tacos, ramen, spritz, taproom.
mcp role *args:
    @case "$1" in restaurant) shift; exec cargo run --quiet --locked -- --share-live --name "Forno Pizza" --port 47831 --plugin restaurant=fixtures/pizzeria.wasm "$@" ;; shoes) shift; exec cargo run --quiet --locked -- --share-live --name "Kiez Kicks" --port 47833 --plugin shoes=fixtures/shoe-store.wasm "$@" ;; guest) shift; exec cargo run --quiet --locked -- --download-dir .hyperlocal/guest-plugins "$@" ;; tacos|ramen|spritz|taproom) cargo build --quiet --locked && exec python3 examples/foodcourt/launch.py "$@" ;; *) echo "Usage: just mcp restaurant|shoes|guest|tacos|ramen|spritz|taproom [server arguments...]" >&2; exit 2 ;; esac

# Open a Codex session for a business or guest. Extra arguments go to Codex.
run role *args:
    @case "$1" in restaurant|shoes|guest|tacos|ramen|spritz|taproom) ;; *) echo "Usage: just run restaurant|shoes|guest|tacos|ramen|spritz|taproom [Codex arguments...]" >&2; exit 2 ;; esac
    @cargo build --quiet --locked
    @exec python3 examples/pizzeria/codex.py "$@"

# Run the interactive restaurant + guest demo (optional: --pizza berghain --table 12).
demo *args:
    @cargo build --quiet --locked
    @python3 examples/pizzeria/demo.py "$@"

# Start the four independent food-court peers together. Enter stop to close them.
foodcourt:
    @cargo build --quiet --locked
    @python3 examples/foodcourt/demo.py
