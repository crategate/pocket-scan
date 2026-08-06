#!/bin/sh
cd "$HOME/pocket-scan"
eval "$(direnv export bash)"
exec ./target/release/pocket-scan
