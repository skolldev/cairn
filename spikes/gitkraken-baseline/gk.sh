#!/bin/zsh
# Open a repo in GitKraken through its own CLI.
A=/Applications/GitKraken.app/Contents
ELECTRON_RUN_AS_NODE=1 exec "$A/MacOS/GitKraken" "$A/Resources/app.asar/src/main/static/cli.js" -p "$1"
