#!/bin/sh
# Build a relocatable app without bundling Valve assets or downloaded community content.
set -eu
cd "$(dirname "$0")/.."
out=${1:-target/dist}
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
app="$out/surf-oss.app"
if [ -e "$app" ] || [ -e "$out/surf-oss-macos.zip" ]; then
    echo "Output already exists. Choose a new output directory; existing files are preserved." >&2
    exit 1
fi
cargo build --locked --release -p surf-app --bin surf-oss
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/assets/fonts/licenses" "$app/Contents/Resources/assets/maps"
cp target/release/surf-oss "$app/Contents/MacOS/surf-oss"
cp LICENSE README.md DEVELOPMENT.md SETUP-VALIDATION.md "$app/Contents/Resources/"
cp assets/maps/manifest.json "$app/Contents/Resources/assets/maps/"
cp assets/fonts/licenses/*.txt "$app/Contents/Resources/assets/fonts/licenses/"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>surf-oss</string>
<key>CFBundleIdentifier</key><string>dev.surf-oss.app</string>
<key>CFBundleName</key><string>surf-oss</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
/usr/bin/codesign --force --sign - "$app"
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$out/surf-oss-macos.zip"
echo "Built $out/surf-oss-macos.zip (ad-hoc signed; not notarized)."
