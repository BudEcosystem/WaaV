"""The desktop's browser, driven over HTTP: a visible Chromium window on the virtual screen.

Every action is done the way a person would do it, so the recording shows the work: a pointer glides
to a link and clicks it, a page is scrolled through while it is read, and what the agent found is
highlighted on the page.

Runs inside the sandbox container. Endpoints (JSON in, JSON out):
  POST /open      {"url": ...}             open a page, return its title and URL
  POST /read      {}                       scroll through the page, return its visible text with title and URL
  POST /click     {"text": ...}            move the pointer to the first link/button with this text and click it
  POST /scroll    {"pixels": 600}          scroll smoothly
  POST /highlight {"text": ...}            scroll to the text and mark it, as a person would point at it
"""
import json
import re
from http.server import BaseHTTPRequestHandler, HTTPServer

from playwright.sync_api import sync_playwright

# A pointer drawn on the page (headless-driven Chromium shows no system cursor), following the mouse.
CURSOR = r"""
(() => {
  if (window.__budCursor) return; window.__budCursor = true;
  const add = () => {
    const c = document.createElement('div');
    c.id = '__bud_cursor';
    // built node by node: pages with Trusted Types (GitHub) refuse innerHTML
    const ns = 'http://www.w3.org/2000/svg', svg = document.createElementNS(ns, 'svg'), path = document.createElementNS(ns, 'path');
    svg.setAttribute('width', '26'); svg.setAttribute('height', '26'); svg.setAttribute('viewBox', '0 0 24 24');
    path.setAttribute('d', 'M4 2l15 9-7 1.6L8.6 20z'); path.setAttribute('fill', '#fff'); path.setAttribute('stroke', '#111');
    path.setAttribute('stroke-width', '1.4'); path.setAttribute('stroke-linejoin', 'round');
    svg.appendChild(path); c.appendChild(svg);
    Object.assign(c.style, {position: 'fixed', left: '480px', top: '420px', zIndex: 2147483647, pointerEvents: 'none',
                            transition: 'transform 90ms ease-out', filter: 'drop-shadow(0 1px 2px rgba(0,0,0,.4))'});
    document.documentElement.appendChild(c);
    addEventListener('mousemove', e => { c.style.left = e.clientX + 'px'; c.style.top = e.clientY + 'px'; }, true);
    addEventListener('mousedown', () => { c.style.transform = 'scale(.82)'; }, true);
    addEventListener('mouseup', () => { c.style.transform = 'scale(1)'; }, true);
  };
  if (document.documentElement) add(); else addEventListener('DOMContentLoaded', add);
})();
"""

pw = sync_playwright().start()
browser = pw.chromium.launch(
    executable_path="/usr/bin/chromium", headless=False,
    args=["--no-sandbox", "--disable-gpu", "--no-first-run", "--window-position=962,0", "--window-size=958,1050",
          "--disable-features=Translate", "--hide-crash-restore-bubble"])
context = browser.new_context(viewport={"width": 958, "height": 960}, device_scale_factor=1)
context.add_init_script(CURSOR)
page = context.new_page()
page.goto("about:blank")
mouse = [480.0, 420.0]


def text_of(limit=7000):
    body = page.inner_text("body", timeout=10000)
    lines = [l.strip() for l in body.splitlines() if l.strip()]
    return "\n".join(lines)[:limit]


def glide(x, y, ms=700):
    """Move the pointer to (x, y) at a person's pace."""
    steps = max(8, int(ms / 16))
    page.mouse.move(x, y, steps=steps)
    mouse[:] = [x, y]


def smooth_scroll(pixels, ms=900):
    steps = max(6, int(ms / 40))
    for _ in range(steps):
        page.mouse.wheel(0, pixels / steps)
        page.wait_for_timeout(40)


def locate(text):
    """The link or button a person would take for `text`: one whose name starts with it, case as
    written ("Releases", not "all releases"), else any that contains it."""
    starts = re.compile(r"^\s*" + re.escape(text))
    for loc in (page.get_by_role("link", name=starts), page.get_by_role("button", name=starts),
                page.get_by_role("link", name=text), page.get_by_role("button", name=text), page.get_by_text(text)):
        visible = [l for l in loc.all() if l.is_visible()]
        if visible:
            return visible[0]
    return page.get_by_text(text).first


def bring_into_view(loc):
    """Scroll smoothly until the element is in the middle third of the window."""
    for _ in range(12):
        box = loc.bounding_box(timeout=8000)
        if box is None:
            break
        mid = box["y"] + box["height"] / 2
        if 240 <= mid <= 720:
            return box
        smooth_scroll(max(-700, min(700, mid - 480)), 450)
    loc.scroll_into_view_if_needed(timeout=8000)
    return loc.bounding_box(timeout=8000)


def handle(path, req):
    if path == "/open":
        page.goto(req["url"], wait_until="domcontentloaded", timeout=30000)
        page.wait_for_timeout(900)
        glide(mouse[0], mouse[1], 50)  # draw the pointer on the new page
        return {"title": page.title(), "url": page.url}
    if path == "/read":
        # Read the way a person does: scroll down through the page, then back to the top.
        height = page.evaluate("document.body.scrollHeight")
        span = max(0, min(int(height) - 900, 2400))
        glide(300, 520, 400)
        if span > 120:
            smooth_scroll(span, 260 * max(3, span // 300))
            page.wait_for_timeout(500)
            smooth_scroll(-span, 900)
        return {"title": page.title(), "url": page.url, "text": text_of(int(req.get("limit", 7000)))}
    if path == "/click":
        loc = locate(req["text"])
        box = bring_into_view(loc)
        if box:
            glide(box["x"] + min(box["width"] / 2, 60), box["y"] + box["height"] / 2, 800)
            loc.evaluate("e => { e.style.outline = '3px solid #8830E8'; e.style.outlineOffset = '2px'; }")
            page.wait_for_timeout(350)
        before = page.url
        loc.click(timeout=8000)
        for _ in range(40):  # a link opens its page: wait for it, not for the old one
            if page.url != before:
                break
            page.wait_for_timeout(100)
        page.wait_for_load_state("domcontentloaded", timeout=20000)
        page.wait_for_timeout(900)
        glide(mouse[0], mouse[1], 50)
        return {"title": page.title(), "url": page.url}
    if path == "/scroll":
        smooth_scroll(int(req.get("pixels", 600)))
        page.wait_for_timeout(300)
        return {"ok": True}
    if path == "/highlight":
        loc = page.get_by_text(req["text"]).first
        box = bring_into_view(loc)
        if box:
            glide(box["x"] + box["width"] + 8, box["y"] + box["height"] / 2, 600)
        loc.evaluate("e => { e.style.background = 'rgba(176,128,240,.35)'; e.style.boxShadow = '0 0 0 3px rgba(136,48,232,.55)';"
                     " e.style.borderRadius = '4px'; }")
        page.wait_for_timeout(700)
        return {"ok": True, "text": loc.inner_text(timeout=4000)[:300]}
    raise ValueError(f"no such endpoint {path}")


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        try:
            out, status = handle(self.path, req), 200
        except Exception as e:  # noqa: BLE001
            out, status = {"error": f"{type(e).__name__}: {e}"[:500]}, 500
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


# Playwright's sync API is single-threaded: serve on this thread, one request at a time.
HTTPServer(("0.0.0.0", 9333), H).serve_forever()
