"""Generate the Yard barn & silo logo SVGs and the web favicon.

Usage: python3 assets/brand/generate.py
"""
import pathlib

out = pathlib.Path(__file__).resolve().parent
favicon_path = out.parents[1] / "web" / "public" / "favicon.svg"

INK, INK_DK, AMBER = "#17201e", "#dce7e3", "#e7b32b"
TILE_DAY, TILE_NIGHT = "#dce2df", "#111715"

# Geometry on a 512 canvas. Barn and silo share a base line at y=448.
BARN = "M40 448V260l44-84 132-64 132 64 44 84v188Z"
SILO = "M352 448V150a60 60 0 0 1 120 0v298Z"
SILO_BANDS = "M352 236h120M352 312h120"
DOOR = "M146 460V312h140v148"
DOOR_Y = "M146 312l70 60 70-60M216 372v88"
PROMPT = "M184 194l24 22-24 22M208 216h44"  # >- : a prompt that is also a sideways Y
CURSOR = "M174 188l30 28-30 28M204 216h52"  # favicon: same >-, bigger


def svg(body, view="0 0 512 512", label="Yard"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view}" role="img" aria-label="{label}">\n'
            f'<title>{label}</title>\n{body}\n</svg>\n')


def mark(ink, prompt_color=None, cut_w=16, prompt_w=16, gap=14, door=True, favicon=False):
    """Barn + silo with knocked-out details. prompt_color=None knocks the prompt out too."""
    round_w = 20  # barn corners are softened with a same-colour round-join stroke
    cuts = []
    if door and not favicon:
        cuts.append(f'<path d="{DOOR}" stroke="#000" stroke-width="{cut_w}" stroke-linecap="round" stroke-linejoin="round" fill="none"/>')
    cuts.append(f'<path d="{DOOR_Y}" stroke="#000" stroke-width="{cut_w}" stroke-linecap="round" stroke-linejoin="round" fill="none"/>')
    loft = CURSOR if favicon else PROMPT
    halo = prompt_w + (12 if prompt_color and ink == INK_DK else 0)
    cuts.append(f'<path d="{loft}" stroke="#000" stroke-width="{halo if prompt_color else prompt_w}" stroke-linecap="round" stroke-linejoin="round" fill="none"/>')
    defs = (
        '<defs>\n'
        f'<mask id="sg" maskUnits="userSpaceOnUse" x="0" y="0" width="512" height="512">'
        f'<rect width="512" height="512" fill="#fff"/>'
        f'<path d="{BARN}" fill="#000" stroke="#000" stroke-width="{round_w + 2 * gap}" stroke-linejoin="round"/>'
        + ('' if favicon else f'<path d="{SILO_BANDS}" stroke="#000" stroke-width="{max(8, cut_w // 2)}"/>') +
        '</mask>\n'
        f'<mask id="bc" maskUnits="userSpaceOnUse" x="0" y="0" width="512" height="512">'
        f'<rect width="512" height="512" fill="#fff"/>{"".join(cuts)}</mask>\n'
        + '</defs>'
    )
    body = (
        f'{defs}\n'
        f'<path d="{SILO}" fill="{ink}" stroke="{ink}" stroke-width="{round_w}" stroke-linejoin="round" mask="url(#sg)"/>\n'
        f'<path d="{BARN}" fill="{ink}" stroke="{ink}" stroke-width="{round_w}" stroke-linejoin="round" mask="url(#bc)"/>'
    )
    if prompt_color:
        body += f'\n<path d="{loft}" stroke="{prompt_color}" stroke-width="{prompt_w}" stroke-linecap="round" stroke-linejoin="round" fill="none"/>'
    return body


VIEW = "24 74 464 390"  # tight to the artwork, including the round-join stroke

files = {
    "yard-barn.svg": svg(mark(INK, AMBER), VIEW),
    "yard-barn-dark.svg": svg(mark(INK_DK, AMBER), VIEW),
    "yard-barn-mono.svg": svg(mark("currentColor"), VIEW),
    "yard-barn-favicon.svg": svg(mark(INK, AMBER, cut_w=40, prompt_w=30, gap=26, favicon=True), VIEW),
    "yard-barn-favicon-dark.svg": svg(mark(INK_DK, AMBER, cut_w=40, prompt_w=30, gap=26, favicon=True), VIEW),
}

# App tile: Herdr-style square with the scene cropped by the frame (bottom and right bleed).
def tile(bg, ink):
    inner = mark(ink, AMBER)
    return svg(f'<rect width="512" height="512" fill="{bg}"/>\n'
               f'<g transform="translate(64 96) scale(1.02)">\n{inner}\n</g>')

files["yard-barn-tile.svg"] = tile(TILE_DAY, INK)
files["yard-barn-tile-dark.svg"] = tile(TILE_NIGHT, INK_DK)

for name, text in files.items():
    (out / name).write_text(text)

# Browser favicon: the favicon cut, switching ink with the tab's colour scheme.
adaptive = files["yard-barn-favicon.svg"].replace(f'fill="{INK}" stroke="{INK}"', 'class="ink"').replace(
    "<defs>",
    f"<style>.ink{{fill:{INK};stroke:{INK}}}"
    f"@media (prefers-color-scheme:dark){{.ink{{fill:{INK_DK};stroke:{INK_DK}}}}}</style>\n<defs>", 1)
favicon_path.write_text(adaptive)
