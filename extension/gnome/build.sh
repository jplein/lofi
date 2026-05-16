#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

rm -rf dist
npx tsc

cp metadata.json dist/
cp dbus-interface.xml dist/

node -e "
const fs = require('fs');
const xml = fs.readFileSync('dbus-interface.xml', 'utf8');
fs.writeFileSync('dist/dbus-xml.js', 'export const dbusXml = ' + JSON.stringify(xml) + ';\n');
"

# Plain zip — gnome-extensions install --force accepts any zip with metadata.json
# at the root. Works equivalently inside the Nix sandbox and the dev shell.
rm -f "lofi-shell@jplein.dev.shell-extension.zip"
(cd dist && zip -r "../lofi-shell@jplein.dev.shell-extension.zip" .)
