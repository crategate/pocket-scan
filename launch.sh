#!/bin/sh
cd "$HOME/pocket-scan"

# secrets (plain export lines, safe to source in any shell)
set -a
. ./.envrc.local
set +a

# everything else from .envrc, inlined (no direnv needed)
export POCKET_SCAN_CONFIG="$PWD/.config"
export POCKET_SCAN_DATA="$PWD/.data"
export POCKET_SCAN_LOG_LEVEL=info
export POCKET_SCAN_DEVICE=/dev/input/by-id/usb-WCM_HIDKeyBoard_00000000011C-event-kbd

exec ./target/release/pocket-scan
