# Icons

Every icon is a hand-authored SVG in `assets/icons/`. Nothing is cropped from the
mockup, nothing is auto-traced, and no raster image is used as an icon.

## Rules

The reference screenshot in `docs/design/` is a *design* reference. Taking its pixels
and turning them into icons would produce artwork nobody drew, at a resolution nobody
chose, with a licence nobody cleared. Icons are drawn from the same geometric language
as the mockup — stroke weight, corner radius, optical size — not from its pixels.

An icon is:

- a single `<svg>` with a `viewBox`, so it scales without a rasteriser;
- drawn on a consistent grid with a consistent stroke width and round caps and joins;
- `fill="none"` with strokes where the shape allows, so it takes the theme's colour;
- free of `<image>`, `<script`, `<foreignObject>` and embedded data URIs;
- named for what it does, in kebab-case: `zoom-in.svg`, `add-sticky-note.svg`.

`cargo xtask icons lint` enforces the mechanical parts: SVG only, a `viewBox`, no
embedded raster or script, drawable content present, and no two files byte-identical
under different names. `cargo xtask icons gallery` writes a contact sheet to
`target/xtask/icon-gallery.html` so the whole set can be reviewed at once, which is the
part no lint can do.

## The set

The shell and its panels need this many before M0 is done. Names are fixed now so the
UI code can refer to them.

**Navigation and document** — `open`, `save`, `save-as`, `print`, `properties`,
`attachments`, `signatures`, `bookmarks`, `layers`, `search`, `find-next`,
`find-previous`

**Pages** — `page-first`, `page-previous`, `page-next`, `page-last`, `zoom-in`,
`zoom-out`, `zoom-fit-width`, `zoom-fit-page`, `rotate-left`, `rotate-right`,
`view-single`, `view-continuous`, `view-two-up`

**Selection and editing** — `select`, `select-text`, `add-text`, `add-image`,
`add-shape`, `add-link`, `crop`, `rotate-object`, `flip-horizontal`, `flip-vertical`,
`opacity`

**Arrange** — `bring-to-front`, `bring-forward`, `send-backward`, `send-to-back`,
`align-left`, `align-centre`, `align-right`, `align-top`, `align-middle`,
`align-bottom`, `distribute-horizontal`, `distribute-vertical`, `group`, `ungroup`

**Colour and stroke** — `colour-fill`, `colour-stroke`, `line-width`, `line-style`,
`opacity`, `blend-mode`

**Annotations** — `highlight`, `underline`, `strikeout`, `squiggly`, `sticky-note`,
`callout`, `free-text`, `line-arrow`, `rectangle`, `ellipse`, `polygon`, `ink`,
`stamp`, `file-attachment`, `comment-reply`, `comment-accepted`,
`comment-rejected`, `comment-completed`, `flatten`

**Forms** — `field-text`, `field-checkbox`, `field-radio`, `field-dropdown`,
`field-listbox`, `field-button`, `field-signature`, `form-prepare`, `form-reset`

**Protect** — `redact`, `redact-search`, `sanitize`, `encrypt`, `unlock`,
`permissions`, `sign`, `signature-valid`, `signature-invalid`

**Organize** — `organize-pages`, `merge`, `split`, `extract-pages`, `insert-blank`,
`import-file`, `page-labels`, `bookmark-add`

**Text editing** — `text-bold`, `text-italic`, `text-underline`, `text-strikethrough`,
`align-justify`, `line-spacing`, `character-spacing`, `baseline-shift`, `font-size`

**Panels and shell** — `panel-thumbnails`, `panel-bookmarks`, `panel-layers`,
`panel-attachments`, `panel-signatures`, `panel-text-properties`, `panel-colour`,
`panel-arrange`, `panel-organize`, `menu`, `close`, `undo`, `redo`, `delete`,
`duplicate`, `settings`, `help`, `warning`, `info`

That is 129 icons. They are drawn to one grid and one stroke weight, and reviewed as a
set in the gallery rather than one at a time.
