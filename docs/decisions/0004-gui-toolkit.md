# ADR-0004: egui and eframe, with every widget painted by us

**Status:** accepted

## Context

The target look is a specific one: a light, dense, macOS-adjacent document tool with a
custom chrome. The reference is a screenshot, not a design system we can implement
against. That rules out any toolkit whose widgets we would have to restyle one at a
time, and it puts a premium on a canvas we can paint exactly.

We also need HiDPI, real IME and keyboard input, a real clipboard, real file dialogs,
and accessibility.

## Decision

`egui` with `eframe` on `winit`, with every widget drawn by us from design tokens.
`egui` is retained-mode immediate mode: there is no widget tree to keep in step with
application state, and layout is code rather than a hierarchy someone else designed.
It renders to a texture we can also hand to our own rasterizer for the page canvas, so
there is one immediate-mode context and one frame loop.

`rfd` for file dialogs and `arboard` for the clipboard. Both are thin wrappers over
platform services; neither is product logic.

The accessibility tree comes from `egui`'s AccessKit integration rather than being
built separately.

## Consequences

**Good.** No native look to fight, which is the whole problem. Everything is a
`Painter` call against a token table, so the mockup is reproducible exactly and themes
are data. HiDPI, IME and input come from the toolkit rather than from us. The icon
renderer and the page renderer can share one rasterizer, which is ADR-0007.

**Bad.** A toolkit that is not designed for a document editor gives no canvas, no
scrolling-with-elasticity model, no deferred rendering, and no way to drive the page
view at 120 Hz while a text cursor blinks at the right subpixel. We will build those.

**Bad.** Immediate mode redraws on every change, so per-frame work must be bounded or
it shows up as latency in the UI thread. The charter's rule that the UI thread never
parses, decodes or rasterizes is what keeps this honest.

**Neutral.** We own the accessibility quality. It is as good as the widgets we declare,
and no better.

## Alternatives

**Iced / GTK / Qt bindings.** GTK and Qt mean a native look we would override, and C
dependencies. Iced is Rust and promising but has no mature accessibility story and a
smaller ecosystem for the dialogs and clipboard we need.

**A custom OpenGL or wgpu renderer for the whole application.** Maximum control, and a
year of building widgets, text layout, IME and accessibility before a window appears.
Not a trade worth making before the product works.
