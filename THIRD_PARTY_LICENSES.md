# Third-party licences

Only permissively licensed material is bundled: OFL, Apache-2.0, MIT, BSD or CC0. No
Adobe branding, icons or proprietary data, and no asset whose licence permits only
personal or non-commercial use.

`cargo xtask policy` checks that every bundled file under `assets/` appears in this
file. An asset with no entry here fails the build.

## Bundled assets

| Asset | Licence | Licence text | Source | Notes |
|---|---|---|---|---|
| `assets/icons/*.svg` | CC0-1.0 | this repository | drawn for this project | Hand-authored. See [ICONS.md](docs/ICONS.md) |
| `assets/theme/tokens.toml` | CC0-1.0 | this repository | sampled from the design reference | Colours and metrics only |
| `assets/fonts/` (the twelve Liberation faces, listed below) | OFL-1.1 | `assets/fonts/OFL-1.1.txt` | Liberation, via the `ttf-liberation` package | Metric-compatible stand-ins for the Standard 14 |

The Liberation faces in `assets/fonts/`:

| File | Stands in for |
|---|---|
| `assets/fonts/LiberationSans-Regular.ttf` | Helvetica |
| `assets/fonts/LiberationSans-Bold.ttf` | Helvetica-Bold |
| `assets/fonts/LiberationSans-Italic.ttf` | Helvetica-Oblique |
| `assets/fonts/LiberationSans-BoldItalic.ttf` | Helvetica-BoldOblique |
| `assets/fonts/LiberationSerif-Regular.ttf` | Times-Roman |
| `assets/fonts/LiberationSerif-Bold.ttf` | Times-Bold |
| `assets/fonts/LiberationSerif-Italic.ttf` | Times-Italic |
| `assets/fonts/LiberationSerif-BoldItalic.ttf` | Times-BoldItalic |
| `assets/fonts/LiberationMono-Regular.ttf` | Courier |
| `assets/fonts/LiberationMono-Bold.ttf` | Courier-Bold |
| `assets/fonts/LiberationMono-Italic.ttf` | Courier-Oblique |
| `assets/fonts/LiberationMono-BoldItalic.ttf` | Courier-BoldOblique |
| `assets/fonts/OFL-1.1.txt` | the licence text itself |

All twelve are TrueType (`glyf`) outlines, unmodified. `OS/2 fsType` is respected at
runtime: a face that forbids embedding is reported to the user, never overridden.

Liberation has no symbol or dingbat face, so `Symbol` and `ZapfDingbats` are reported
rather than substituted. See `crates/mangle-font/src/substitutes.rs`.

## Fonts

Liberation is bundled — see the tables above. The rest, with the licence each will carry:

| Font | Licence | Why it is needed |
|---|---|---|
| Inter | OFL-1.1 | The interface font |
| ~~Liberation~~ | OFL-1.1 | Bundled: metric-compatible stand-ins for the Standard 14, so an unembedded Helvetica renders at the right width |
| Playfair Display | OFL-1.1 | The mockup's display face, and the subsetting test fixture in `FINISH.md` F31 |
| A symbol or dingbat face | OFL-1.1 | ZapfDingbats and the annotation set |
| CJK fallbacks | OFL-1.1 or the system's own | Only if system coverage is insufficient on a supported platform |

An `OS/2 fsType` that forbids embedding is respected at runtime and reported to the
user rather than overridden.

## CMaps

Predefined CMaps and encoding tables are a separate question from fonts: they are
Adobe's, distributed under a specific licence. Nothing is bundled yet. When it is, this
file records the exact resource, its licence, and the distribution terms. A file that
cannot be redistributed is not bundled, and its absence is surfaced in the UI rather
than worked around silently.

## Why the table matters

Every row is a licence the project is obliged to keep. A bundled asset that is not
listed here is either a licence violation or an oversight, and the policy check turns
the second into a build failure so it cannot reach a release by accident.
