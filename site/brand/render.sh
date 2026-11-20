#!/bin/sh
# Renders the PNGs in img/ from the templates beside this script, in a
# headless browser (agent-browser). Run it from site/ after changing
# img/favicon.svg or brand/og.html, and commit what it writes.
set -eu
cd "$(dirname "$0")/.."
python3 -m http.server 8799 --bind 127.0.0.1 >/dev/null 2>&1 &
server=$!
trap 'kill "$server"' EXIT
sleep 1
export AGENT_BROWSER_SESSION=mokuji-brand
agent-browser set media light >/dev/null
shot() { # url width height out
  agent-browser set viewport "$2" "$3" >/dev/null
  agent-browser open "http://127.0.0.1:8799/$1" >/dev/null
  agent-browser wait 1500 >/dev/null
  agent-browser screenshot "$4" >/dev/null
  echo "$4"
}
shot "brand/og.html" 1200 630 img/og-en.png
shot "brand/og.html?lang=ja" 1200 630 img/og-ja.png
shot "brand/icon.html?size=180" 180 180 img/apple-touch-icon.png
shot "brand/icon.html?size=32" 32 32 img/favicon-32.png
agent-browser close >/dev/null
