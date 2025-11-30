#!/usr/bin/env bash
# Convenience helper script for installing aiurgaze for the current user.
#
# This is *not* required; the official way is:
#   cargo install --path .
#   mkdir -p ~/.config/aiurgaze ~/.local/share/aiurgaze
#   cp ./config.toml ~/.config/aiurgaze/config.toml
#   cp -r ./assets ~/.local/share/aiurgaze/
#   cp -r ./data ~/.local/share/aiurgaze/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "==> Installing aiurgaze for current user"
echo "Using cargo install --path . to build and install the binary."

cargo install --path . --locked

get_xdg_or_fallback() {
    # $1 = config|data
    local key
    if [ "$1" = "config" ]; then
        key="XDG_CONFIG_HOME"
        fallback="$HOME/.config"
    else
        key="XDG_DATA_HOME"
        fallback="$HOME/.local/share"
    fi
    # Use bash indirect expansion to read the env var named by $key
    val="${!key:-}"
    if [ -n "$val" ] && echo "$val" | grep -q "/snap/"; then
        echo "$fallback"
    else
        if [ -n "$val" ]; then
            echo "$val"
        else
            echo "$fallback"
        fi
    fi
}

CONFIG_HOME="$(get_xdg_or_fallback config)"
DATA_HOME="$(get_xdg_or_fallback data)"

echo ""
echo "==> Installing configuration file"
mkdir -p "$CONFIG_HOME/aiurgaze"
if [ -f "config.toml" ]; then
    cp -v "config.toml" "$CONFIG_HOME/aiurgaze/config.toml"
else
    echo "Warning: config.toml not found in repo root; no config copied."
fi

echo ""
echo "==> Installing assets and data"
mkdir -p "$DATA_HOME/aiurgaze"

for d in assets data maps; do
    if [ -d "$d" ]; then
        rm -rf "$DATA_HOME/aiurgaze/$d"
        cp -rv "$d" "$DATA_HOME/aiurgaze/"
    else
        echo "Warning: $d directory not found; no $d copied."
    fi
done

echo ""
echo "==> Installation complete!"
echo "Binary:  $(command -v aiurgaze || echo '\"aiurgaze\" not on PATH')"
echo "Config:  $CONFIG_HOME/aiurgaze/config.toml"
echo "Assets:  $DATA_HOME/aiurgaze/assets/"
echo "Data:    $DATA_HOME/aiurgaze/data/"
echo "Maps:    $DATA_HOME/aiurgaze/maps/"
echo ""
echo "You can now run: aiurgaze"
