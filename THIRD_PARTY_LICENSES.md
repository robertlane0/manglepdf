# Third-party licences

Only permissively licensed material is bundled: OFL, Apache-2.0, MIT, BSD or CC0. No
Adobe branding, icons or proprietary data, and no asset whose licence permits only
personal or non-commercial use.

`cargo xtask policy` checks that every bundled file under `assets/` appears in this
file. An asset with no entry here fails the build.

## Bundled assets

None yet. The table below is the contract for what may be added.

| Asset | Licence | Licence text | Source | Notes |
|---|---|---|---|---|
| `assets/icons/*.svg` | CC0-1.0 | this repository | drawn for this project | Hand-authored. See [ICONS.md](docs/ICONS.md) |
| `assets/theme/tokens.toml` | CC0-1.0 | this repository | sampled from the design reference | Colours and metrics only |

## Fonts

Not yet bundled. The expected set, with the licence each will carry:

| Font | Licence | Why it is needed |
|---|---|---|
| Inter | OFL-1.1 | The interface font |
| Liberation | OFL-1.1 | Metric-compatible stand-ins for the Standard 14, so an unembedded Helvetica renders at the right width |
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
