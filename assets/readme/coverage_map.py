"""Draw the README's regional-coverage map: a dot map of the world (Natural Earth 1:110m, from
world-atlas), the countries of the regional providers tinted, and a card per region.

usage: python3 -I coverage_map.py countries-110m.json coverage-map.svg

countries-110m.json is https://cdn.jsdelivr.net/npm/world-atlas@2.0.2/countries-110m.json. Edit REGIONS and
GLOBAL when a provider is added; the assert keeps the vendor count honest.
"""
import json
import sys

topo = json.load(open(sys.argv[1]))
sx, sy = topo["transform"]["scale"]
tx, ty = topo["transform"]["translate"]

arcs = []
for arc in topo["arcs"]:
    x = y = 0
    pts = []
    for dx, dy in arc:
        x += dx
        y += dy
        pts.append((x * sx + tx, y * sy + ty))
    arcs.append(pts)


def ring(indices):
    out = []
    for i in indices:
        pts = arcs[i] if i >= 0 else arcs[~i][::-1]
        out.extend(pts if not out else pts[1:])
    # A ring that crosses the antimeridian (Russia's Chukotka, Fiji) is unwrapped east of 180.
    if max(x for x, _ in out) - min(x for x, _ in out) > 180:
        out = [(x + 360 if x < 0 else x, y) for x, y in out]
    return out


countries = {}
for g in topo["objects"]["countries"]["geometries"]:
    polys = []
    if g["type"] == "Polygon":
        polys = [[ring(r) for r in g["arcs"]]]
    elif g["type"] == "MultiPolygon":
        polys = [[ring(r) for r in p] for p in g["arcs"]]
    countries[g.get("id") or g["properties"]["name"]] = (g["properties"]["name"], polys)


def inside(lon, lat, rings):
    hit = False
    for r in rings:
        j = len(r) - 1
        for i in range(len(r)):
            xi, yi = r[i]
            xj, yj = r[j]
            if (yi > lat) != (yj > lat) and lon < (xj - xi) * (lat - yi) / (yj - yi) + xi:
                hit = not hit
            j = i
    return hit


boxes = {}
for cid, (_, polys) in countries.items():
    for k, p in enumerate(polys):
        xs = [x for r in p for x, _ in r]
        ys = [y for r in p for _, y in r]
        boxes[(cid, k)] = (min(xs), max(xs), min(ys), max(ys))


def country_at(lon, lat):
    for (cid, k), (x0, x1, y0, y1) in boxes.items():
        for x in (lon, lon + 360):
            if x0 <= x <= x1 and y0 <= lat <= y1 and inside(x, lat, countries[cid][1][k]):
                return cid
    return None


# Regions: the countries a regional provider comes from, its colour, its card.
REGIONS = [
    ("Europe", "#60A5FA", ["826", "250", "203", "056"], "55+ to 100+ languages, code-switching, Celtic voices",
     ["Speechmatics (UK)", "Gladia (France)", "Phonexia (Czechia)", "Acapela (Belgium)", "CereProc (Scotland)"]),
    ("Russia", "#F472B6", ["643"], "Russian, with emotional voices",
     ["Yandex SpeechKit", "T-Bank VoiceKit", "SberDevices SaluteSpeech"]),
    ("India", "#FBBF24", ["356"], "22 Indian languages, code-mixed Hinglish",
     ["Sarvam AI", "Gnani.ai", "Reverie", "Bhashini (AI4Bharat)", "Smallest.ai"]),
    ("China", "#F87171", ["156"], "Mandarin, Cantonese and regional dialects",
     ["iFlytek", "Alibaba Cloud", "Baidu AI", "Tencent Cloud", "Huawei Cloud"]),
    ("Korea & Japan", "#34D399", ["410", "392"], "Korean, Japanese",
     ["NAVER CLOVA (Korea)", "AmiVoice (Japan)"]),
    ("South-East Asia", "#C084FC", ["704", "764"], "Vietnamese accents, Thai",
     ["Zalo AI (Vietnam)", "FPT.AI (Vietnam)", "Viettel AI (Vietnam)", "NECTEC (Thailand)"]),
]
GLOBAL = ["Deepgram", "OpenAI", "Google", "Microsoft Azure", "AWS", "ElevenLabs", "AssemblyAI", "Cartesia",
          "IBM Watson", "Groq", "Rev AI", "Hume", "Resemble", "Speechify", "WellSaid", "Murf", "Unreal Speech",
          "Inworld", "Ultravox", "xAI Grok"]
colour_of = {c: col for _, col, cs, _, _ in REGIONS for c in cs}
# Where the tint applies: a country's overseas land (French Guiana) is not its region.
TINT_BOX = {"826": (-12, 3, 49, 61), "250": (-6, 10, 41, 52), "203": (11, 20, 48, 52), "056": (2, 7, 49, 52)}
assert sum(len(r[4]) for r in REGIONS) + len(GLOBAL) == 44, "44 vendors"

W, X0, Y0, PX = 1280, 40, 116, 1200 / 360
LAT_TOP, LAT_BOTTOM, STEP = 78, -56, 1.6


def esc(t):
    return t.replace("&", "&amp;").replace("<", "&lt;")


def xy(lon, lat):
    return X0 + (lon + 180) * PX, Y0 + (LAT_TOP - lat) * PX


MAP_BOTTOM = xy(0, LAT_BOTTOM)[1]
out = []
add = out.append
H = int(MAP_BOTTOM) + 330
add(f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" role="img" aria-labelledby="t d">')
add('<title id="t">Regional coverage of WaaV providers</title>')
add('<desc id="d">A world map. Tinted countries are home to regional speech providers integrated in WaaV: '
    'Europe, Russia, India, China, Korea and Japan, Vietnam and Thailand. Cards list each region\'s providers '
    'and languages; twenty global cloud vendors cover the rest.</desc>')
add('''<defs>
  <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#120B32"/><stop offset="0.6" stop-color="#17123F"/><stop offset="1" stop-color="#0A2540"/></linearGradient>
  <radialGradient id="halo" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#FFFFFF" stop-opacity="0.35"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/></radialGradient>
  <clipPath id="card"><rect width="%d" height="%d" rx="24"/></clipPath>
</defs>''' % (W, H))
add(f'<g clip-path="url(#card)"><rect width="{W}" height="{H}" fill="url(#bg)"/></g>')
add('<g font-family="Inter, -apple-system, \'Segoe UI\', Helvetica, Arial, sans-serif">')
add('<text x="40" y="54" font-size="24" font-weight="800" fill="#FFFFFF">Speak your callers’ language</text>')
add('<text x="40" y="82" font-size="15" fill="#C7D2FE" fill-opacity="0.9">44 vendors behind one API: the global clouds, plus regional specialists for the languages, accents and dialects they know best.</text>')

# dots
base, tinted = [], {}
lat = LAT_TOP - STEP / 2
while lat > LAT_BOTTOM:
    lon = -180 + STEP / 2
    while lon < 180:
        cid = country_at(lon, lat)
        if cid is not None:
            x, y = xy(lon, lat)
            c = colour_of.get(cid)
            box = TINT_BOX.get(cid)
            if c and box and not (box[0] <= lon <= box[1] and box[2] <= lat <= box[3]):
                c = None
            (tinted.setdefault(c, []) if c else base).append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="1.9"/>')
        lon += STEP
    lat -= STEP
add('<g fill="#475569" fill-opacity="0.55">' + "".join(base) + "</g>")
for c, dots in tinted.items():
    add(f'<g fill="{c}">' + "".join(dots) + "</g>")

# pins: a halo, a pulse and the region's label
PINS = {
    "Europe": ((5, 50), "end", -20, -14),
    "Russia": ((60, 58), "start", 14, -10),
    "India": ((78.5, 22), "end", -14, 24),
    "China": ((103, 35), "start", 14, -12),
    "Korea & Japan": ((133, 37), "start", 14, -8),
    "South-East Asia": ((103, 15), "start", 16, 20),
}
for name, col, _, _, provs in REGIONS:
    (lon, lat), anchor, dx, dy = PINS[name]
    x, y = xy(lon, lat)
    add(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="16" fill="url(#halo)"/>')
    add(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="5" fill="{col}" stroke="#FFFFFF" stroke-width="2"/>')
    add(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="5" fill="none" stroke="{col}" stroke-width="2">'
        '<animate attributeName="r" values="5;16;5" dur="2.6s" repeatCount="indefinite"/>'
        '<animate attributeName="opacity" values="0.9;0;0.9" dur="2.6s" repeatCount="indefinite"/></circle>')
    add(f'<text x="{x + dx:.1f}" y="{y + dy:.1f}" text-anchor="{anchor}" font-size="13" font-weight="700" fill="#FFFFFF" '
        f'paint-order="stroke" stroke="#120B32" stroke-width="4" stroke-linejoin="round">{esc(name)} · {len(provs)}</text>')

# cards
cw, gap = 192, 9.6
cy = MAP_BOTTOM + 14
for i, (name, col, _, langs, provs) in enumerate(REGIONS):
    x = 40 + i * (cw + gap)
    add(f'<g transform="translate({x:.1f} {cy:.1f})">')
    add(f'<rect width="{cw}" height="196" rx="14" fill="#FFFFFF" fill-opacity="0.06" stroke="{col}" stroke-opacity="0.55"/>')
    add(f'<rect x="0" y="0" width="{cw}" height="4" rx="2" fill="{col}"/>')
    add(f'<text x="14" y="30" font-size="15" font-weight="800" fill="#FFFFFF">{esc(name)}</text>')
    # the languages line, wrapped by hand at about 30 characters
    words, lines, cur = langs.split(), [], ""
    for w in words:
        if len(cur) + len(w) + 1 > 30:
            lines.append(cur)
            cur = w
        else:
            cur = (cur + " " + w).strip()
    lines.append(cur)
    for k, line in enumerate(lines):
        add(f'<text x="14" y="{50 + k * 15}" font-size="11.5" fill="{col}">{line}</text>')
    top = 50 + len(lines) * 15 + 10
    for k, p in enumerate(provs):
        add(f'<text x="14" y="{top + k * 21}" font-size="13" fill="#E2E8F0">{esc(p)}</text>')
    add("</g>")

gy = cy + 212
add(f'<g transform="translate(40 {gy:.1f})">')
add('<rect width="1200" height="72" rx="14" fill="#FFFFFF" fill-opacity="0.04" stroke="#FFFFFF" stroke-opacity="0.15"/>')
add('<text x="16" y="28" font-size="14" font-weight="800" fill="#FFFFFF">Global clouds · 20</text>')
add('<text x="16" y="48" font-size="11.5" fill="#94A3B8">100+ languages between them</text>')
add('<text x="250" y="31" font-size="13" fill="#E2E8F0">' + " · ".join(GLOBAL[:10]) + "</text>")
add('<text x="250" y="53" font-size="13" fill="#E2E8F0">' + " · ".join(GLOBAL[10:]) + "</text>")
add("</g>")
add(f'<text x="{W - 40}" y="{H - 16}" text-anchor="end" font-size="11" fill="#64748B">Map: Natural Earth (public domain). '
    'Tinted: where a regional provider comes from. 31 speech-to-text, 34 text-to-speech and 11 speech-to-speech integrations.</text>')
add("</g></svg>")
open(sys.argv[2], "w").write("\n".join(out))
print("dots", len(base) + sum(len(v) for v in tinted.values()), "tinted", {k: len(v) for k, v in tinted.items()})
