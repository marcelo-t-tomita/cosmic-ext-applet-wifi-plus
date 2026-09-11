#!/usr/bin/env bash
set -euo pipefail

APP_ID="io.github.marcelo_t_tomita.CosmicWifiPlus"
PANEL_CONFIG="$HOME/.config/cosmic/com.system76.CosmicPanel.Panel/v1/plugins_wings"

rm -f "$HOME/.local/bin/cosmic-wifi-plus"
rm -f "$HOME/.local/share/applications/$APP_ID.desktop"

if [[ -f $PANEL_CONFIG ]]; then
  cp "$PANEL_CONFIG" "$PANEL_CONFIG.bak.$(date +%s)"
  python3 - "$PANEL_CONFIG" "$APP_ID" <<'PY'
import re, sys

path, app_id = sys.argv[1], sys.argv[2]
config = open(path).read()
config = re.sub(rf'\s*"{re.escape(app_id)}",?', '', config)
open(path, "w").write(config)
PY
fi

pkill -x cosmic-panel || true
echo "Removed."
