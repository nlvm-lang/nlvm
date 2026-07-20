# NL brand assets

Identity assets for the NL language and the nlvm project. Intended to move to
the `nlvm-lang` GitHub organization (e.g. a `.github` or `brand` repository)
once the repositories are transferred there.

## Files

- `logo.svg` — master logo (512×512 viewBox, letterforms drawn as paths, no
  font dependency). Edit this one; regenerate the PNGs from it.
- `logo-{1024,512,256,128,64,32,16}.png` — raster exports.
  - **GitHub org/repo avatar:** use `logo-512.png` (or `logo-1024.png`).
  - **Favicon:** the site links `logo.svg` directly; `logo-32.png` / `logo-16.png`
    are fallbacks for contexts without SVG support.

## Colors

Taken from the site palette (`docs/assets/style.css`):

- Background: `#0c1110`
- Letterforms: `#3ecfae` (primary)
- Border: `#2e423b` (border-strong)
- Glow tint: `#3ecfae` at 16% opacity

## Regenerating the PNGs

```sh
pip install cairosvg  # in a venv
python -c "import cairosvg; [cairosvg.svg2png(url='logo.svg', write_to=f'logo-{s}.png', output_width=s, output_height=s) for s in (1024, 512, 256, 128, 64, 32, 16)]"
```
