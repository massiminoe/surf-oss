# surf-oss app icon

The official icon is a lowercase, two-line wordmark: `surf` over `oss`.
Both lines share a right edge, naturally indenting the shorter second line.

- Typeface: Helvetica Neue Bold, converted to outlines by CoreText during generation.
- Background: midnight navy `#071130`.
- `surf`: warm off-white `#F9F7EF`.
- `oss`: cyan `#0CD7ED`.
- Canvas: 1024 square; rounded tile inset 64, corner radius 204.
- Transparent outside the tile; no baked-in outer shadow.

`surf-oss.png` is the full-size artwork. `surf-oss.icns` contains the standard
16–1024 pixel macOS representations, each rendered directly from the outlines.
The packaging script copies the ICNS into the app and declares it in Info.plist.

Regenerate from the repository root on macOS:

```sh
swift tools/make_icon.swift /tmp/surf-oss.iconset
```

The generator uses macOS AppKit/CoreText and the system font; no font file is
redistributed.
