#!/usr/bin/env bash
# Installs the applet and registers it with cosmic-panel.

set -euo pipefail

APP_ID="io.github.marcelo_t_tomita.CosmicWifiPlus"
BIN_NAME="cosmic-wifi-plus"
BIN_DIR="$HOME/.local/bin"
DESKTOP_DIR="$HOME/.local/share/applications"
PANEL_CONFIG="$HOME/.config/cosmic/com.system76.CosmicPanel.Panel/v1/plugins_wings"

root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

if [[ ! -x "$root/target/release/$BIN_NAME" ]]; then
  echo "Build it first: cargo build --release" >&2
  exit 1
fi

mkdir -p "$BIN_DIR" "$DESKTOP_DIR"
install -m 755 "$root/target/release/$BIN_NAME" "$BIN_DIR/$BIN_NAME"

# The panel launches applets by Exec, and its own PATH does not necessarily
# include ~/.local/bin, so the entry carries an absolute path.
sed "s|^Exec=.*|Exec=$BIN_DIR/$BIN_NAME|" "$root/data/$APP_ID.desktop" \
  >"$DESKTOP_DIR/$APP_ID.desktop"

if [[ ! -f $PANEL_CONFIG ]]; then
  echo "No panel config at $PANEL_CONFIG -- add '$APP_ID' to the panel by hand." >&2
  exit 1
fi

if grep -q "$APP_ID" "$PANEL_CONFIG"; then
  echo "Already registered with the panel."
else
  cp "$PANEL_CONFIG" "$PANEL_CONFIG.bak.$(date +%s)"

  # Sits immediately before the stock network applet in the right-hand wing.
  python3 - "$PANEL_CONFIG" "$APP_ID" <<'PY'
import sys

path, app_id = sys.argv[1], sys.argv[2]
config = open(path).read()
anchor = '"com.system76.CosmicAppletNetwork"'

if anchor in config:
    config = config.replace(anchor, f'"{app_id}",\n    {anchor}', 1)
else:
    # No stock network applet: append to the right-hand wing instead.
    index = config.rindex("]")
    config = config[:index] + f',\n    "{app_id}"\n' + config[index:]

open(path, "w").write(config)
PY
  echo "Registered with the panel."
fi

echo
echo "Restarting cosmic-panel..."
pkill -x cosmic-panel || true
echo "Done. The applet appears next to the stock network icon."
echo "To remove it: run ./uninstall.sh"
