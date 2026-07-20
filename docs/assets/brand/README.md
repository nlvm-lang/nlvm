# NL brand assets

Identity assets for the NL language and the nlvm project. Intended to move to
the `nlvm-lang` GitHub organization (e.g. a `.github` or `brand` repository)
once the repositories are transferred there.

## Files

- `generate.py` — the source of truth. Builds `logo.svg`, all `logo-*.png`
  sizes, and `social-preview.{svg,png}` from one set of constants (colors,
  the icon path data, layout). Edit this file, not the generated SVGs
  directly — a re-run overwrites them.
- `logo.svg` — master logo (512×512, letterforms drawn as paths, no font
  dependency — this one renders identically everywhere).
- `logo-{1024,512,256,128,64,32,16}.png` — raster exports.
  - **GitHub org/repo avatar:** use `logo-512.png` (or `logo-1024.png`).
  - **Favicon:** the site links `logo.svg` directly; `logo-32.png` / `logo-16.png`
    are fallbacks for contexts without SVG support.
- `social-preview.svg` / `social-preview.png` — 1280×640 social card (GitHub
  org/repo social preview, Open Graph image).

## Typography

Two families, matching the site's own `--mono` / `--sans` split in
`../style.css` — not arbitrary fallback fonts:

- **JetBrains Mono** for anything brand or code: the wordmark, the eyebrow
  line. First name in the site's `--mono` stack.
- **Inter** for running text (the tagline). Stands in for the site's
  `--sans` stack (`system-ui`, `-apple-system`, `Segoe UI`, `Roboto`) — a
  neutral, professional match rather than the Linux default fallback
  (Liberation Sans / DejaVu Sans), which is what `generate.py` used to
  fall back to before these were installed locally.

If `fc-list | grep -iE "jetbrains mono|inter"` comes up empty on the machine
you're regenerating from, install both first — see the docstring at the top
of `generate.py` for the exact commands (no root needed, fonts go in
`~/.local/share/fonts`).

**Why the `.svg` and `.png` can look different when opened locally:** an SVG
`<text>` element only stores a font *name* — each viewer resolves that name
against its own system's fonts at render time. `social-preview.png` is
already resolved (JetBrains Mono / Inter were installed when it was
generated, and the result is baked into pixels), so it always looks the
same. `social-preview.svg` re-resolves the fonts every time it's opened —
if JetBrains Mono / Inter aren't installed on whatever machine or viewer
opens it, it silently falls back to that system's default sans-serif, and
the "one custom mark, two deliberate type families" system is gone. This is
normal SVG behavior, not a generation bug — always ship/link the `.png` for
anything you don't control the rendering environment of (GitHub social
preview, the site's `og:image`); treat the `.svg` as source to regenerate
from, not a portable artifact in its own right. `logo.svg` doesn't have this
problem since it has no text at all.

## Colors

Taken from the site palette (`docs/assets/style.css`):

- Background: `#0c1110`
- Letterforms / primary: `#3ecfae`
- Border: `#2e423b` (border-strong)
- Muted text: `#90a49d` · faint text: `#5f7370`
- Glow tint: `#3ecfae` at 9–16% opacity

## Regenerating everything

```sh
pip install cairosvg  # in a venv
python3 generate.py
```

or with `uv`

```
uv init
uv venv .venv
uv pip install cairosvg
uv run python generate.py
```