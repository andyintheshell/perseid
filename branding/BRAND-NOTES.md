# Perseid brand notes

**Name:** Perseid
**Tagline:** INSPECT · MODIFY · REPLAY
**Descriptor:** A lightweight web security workbench.

**Primary palette**

| Color | Hex | Use |
| --- | --- | --- |
| Navy | `#112D68` | Wordmark, mark strokes, headlines |
| Navy dark | `#0B235A` | Deep shading and icon depth |
| Amber | `#FFAF1A` | Meteor streaks, secondary accent |
| Orange | `#FF9915` | Meteor streak, primary accent |
| Coral | `#FF4D2B` | Tagline and the leading meteor |
| Paper | `#FCFBF8` | Light surface |
| Slate | `#6B7280` | Body text |

The icon is a meteor-shower symbol: three navy streaks, one amber-to-coral
streak ending in a large meteor, and a single small dot.

**Typography:** Inter (or a system sans-serif fallback). The wordmark uses a
heavy weight, the tagline is all caps in coral, and body copy is slate. No fonts
are bundled.

**Files**

- `perseid-readme-banner.svg` / `.png`: README hero (1600×500).
- `perseid-lockup.svg` / `.png`: mark, wordmark, tagline, and descriptor
  (1600×430); the app's About window embeds the PNG.
- `perseid-github-social.svg`: source for the GitHub social card; upload
  `perseid-social-preview.png` (1280×640) in the repository settings.
- `perseid-symbol.svg`, `perseid-transparent-mark.png`: transparent mark for
  flexible placement.
- `perseid-icon-tile.svg`, `perseid-tile-icon.png`: rounded app tile.
- `perseid-icon-white.svg`, `perseid-primary-icon.png`: primary icon on white.
- `perseid-wordmark.svg` / `.png`: wordmark without the mark.
- `perseid-icon-512.png`, `perseid-icon-128.png`, `perseid-icon-32.png`: raster
  app icons exported from the platform kit.

The application embeds packaged copies under `crates/perseid/assets/`:
`icon-128.png` (window icon), `logo-light.png` (About lockup), and `mark.png`
(toolbar mark). Keep those in sync when the brand assets change.
- `perseid-avatar-512.png`: GitHub profile/avatar image (512×512); upload it
  manually in the GitHub account or organization settings.
