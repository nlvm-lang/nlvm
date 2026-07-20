#!/usr/bin/env python3
"""Generates all NL brand assets (logo, favicons, social preview) from one
source of truth. Edit the constants below, then run:

    pip install cairosvg   # once, ideally in a venv
    python3 generate.py

Typography: JetBrains Mono for anything brand/code (matches --mono in
../style.css) and Inter for running text (matches the --sans stack's
system-ui/Roboto intent). Both are Google/JetBrains open fonts; install them
locally before running if `fc-list | grep -i "jetbrains mono\\|inter"` comes
up empty:

    curl -sL -o /tmp/jbm.zip https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip
    curl -sL -o /tmp/inter.zip https://github.com/rsms/inter/releases/download/v4.0/Inter-4.0.zip
    mkdir -p ~/.local/share/fonts/nlvm-brand && cd /tmp
    unzip -o jbm.zip -d jbm && unzip -o inter.zip -d inter
    cp jbm/fonts/ttf/JetBrainsMono-{Regular,Medium,Bold}.ttf ~/.local/share/fonts/nlvm-brand/
    cp inter/extras/ttf/Inter-{Regular,Medium,SemiBold,Bold}.ttf ~/.local/share/fonts/nlvm-brand/
    fc-cache -f ~/.local/share/fonts/nlvm-brand
"""
import cairosvg

BG = "#0c1110"
BORDER = "#2e423b"
PRIMARY = "#3ecfae"
TEXT = "#dbe4e0"
TEXT_MUTED = "#90a49d"
TEXT_FAINT = "#5f7370"

MONO = "JetBrains Mono"
SANS = "Inter"

ICON_SIZES = (1024, 512, 256, 128, 64, 32, 16)


def logo_mark(x=0, y=0, scale=1.0) -> str:
    """The 'nl' pictogram as drawn paths (no font dependency) — the one
    fixed element every other asset is built around."""
    return f'''<g transform="translate({x} {y}) scale({scale})">
    <rect width="512" height="512" rx="115" fill="{BG}"/>
    <rect width="512" height="512" rx="115" fill="url(#tint)"/>
    <rect x="9" y="9" width="494" height="494" rx="107" fill="none" stroke="{BORDER}" stroke-width="10"/>
    <g fill="none" stroke="{PRIMARY}" stroke-width="46" stroke-linecap="round">
      <path d="M163 364 V174"/>
      <path d="M163 232 A58 58 0 0 1 279 232 V364"/>
      <path d="M349 364 V128"/>
    </g>
  </g>'''


def build_logo_svg() -> str:
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <defs>
    <radialGradient id="tint" cx="0.5" cy="0.38" r="0.72">
      <stop offset="0" stop-color="{PRIMARY}" stop-opacity="0.16"/>
      <stop offset="1" stop-color="{PRIMARY}" stop-opacity="0"/>
    </radialGradient>
  </defs>
  {logo_mark()}
</svg>
'''


def build_social_preview_svg() -> str:
    icon_x, icon_y, icon_scale = 230, 232, 0.5078  # 260px icon, vertically centered
    text_x = 562
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1280 640" width="1280" height="640">
  <defs>
    <radialGradient id="glow" cx="0.5" cy="0.5" r="0.55">
      <stop offset="0" stop-color="{PRIMARY}" stop-opacity="0.09"/>
      <stop offset="1" stop-color="{PRIMARY}" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="tint" cx="0.5" cy="0.38" r="0.72">
      <stop offset="0" stop-color="{PRIMARY}" stop-opacity="0.16"/>
      <stop offset="1" stop-color="{PRIMARY}" stop-opacity="0"/>
    </radialGradient>
  </defs>

  <rect width="1280" height="640" fill="{BG}"/>
  <rect width="1280" height="640" fill="url(#glow)"/>

  {logo_mark(icon_x, icon_y, icon_scale)}

  <g font-family="{MONO}">
    <text x="{text_x}" y="250" font-size="18" font-weight="500" letter-spacing="4" fill="{PRIMARY}">THE NL PROGRAMMING LANGUAGE</text>
    <text x="{text_x - 6}" y="374" font-size="104" font-weight="700">
      <tspan fill="{TEXT}">nl</tspan><tspan fill="{TEXT_FAINT}">vm</tspan><tspan fill="{PRIMARY}">_</tspan>
    </text>
  </g>
  <g font-family="{MONO}" font-size="32" font-weight="700" fill="{TEXT}">
    <text x="{text_x}" y="446">A small language that takes</text>
    <text x="{text_x}" y="490" xml:space="preserve"><tspan fill="{PRIMARY}">correctness</tspan><tspan fill="{TEXT_MUTED}"> seriously.</tspan></text>
  </g>
</svg>
'''


def main():
    logo_svg = build_logo_svg()
    with open("logo.svg", "w") as f:
        f.write(logo_svg)
    for size in ICON_SIZES:
        cairosvg.svg2png(bytestring=logo_svg.encode(), write_to=f"logo-{size}.png",
                          output_width=size, output_height=size)

    social_svg = build_social_preview_svg()
    with open("social-preview.svg", "w") as f:
        f.write(social_svg)
    cairosvg.svg2png(bytestring=social_svg.encode(), write_to="social-preview.png",
                      output_width=1280, output_height=640)

    print("Generated logo.svg, logo-{" + ",".join(map(str, ICON_SIZES)) + "}.png, social-preview.{svg,png}")


if __name__ == "__main__":
    main()
