# /// script
# requires-python = ">=3.11"
# dependencies = ["markdown-it-py>=3", "mdit-py-plugins>=0.4", "pygments>=2.18"]
# ///
"""Build the documentation site (GitHub Pages) from the repository's Markdown.

    uv run tools/build_docs.py site        # then open site/index.html
    uv run tools/build_docs.py site --update-fixture   # after changing themes/gallery/

The Markdown stays the single source: README.md (minus its TODO list, which
becomes the roadmap page), docs/ARCHITECTURE.md, docs/EXTENDING.md,
docs/MARKETS-PLAN.md, docs/RELEASES.md and themes/README.md, plus a card for every theme and the
gallery's files to download. Links between them become links between pages;
links to other files point at GitHub. The page is styled with the Everforge palette and fonts.
`.github/workflows/docs.yml` runs this and deploys the result.
"""

from __future__ import annotations

import html
import json
import posixpath
import re
import shutil
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from markdown_it import MarkdownIt
from mdit_py_plugins.anchors import anchors_plugin
from mdit_py_plugins.tasklists import tasklists_plugin
from pygments import highlight
from pygments.formatters import HtmlFormatter
from pygments.lexers import get_lexer_by_name
from pygments.util import ClassNotFound

ROOT = Path(__file__).resolve().parent.parent
REPO = "https://github.com/ArenKDesai/miso-terminal"
BLOB = f"{REPO}/blob/main/"
TREE = f"{REPO}/tree/main/"
MARKER = ".build_docs"  # marks an output directory as ours to replace
# The recorded gallery index for offline mode and the tests (keep in step with themes/gallery/).
FIXTURE = ROOT / "fixtures/arenkdesai.github.io/miso-terminal/themes/index.json"


@dataclass
class Page:
    slug: str  # output file
    source: str  # repo path of the Markdown
    nav: str  # label in the top bar
    markdown: str = ""
    html: str = ""
    title: str = ""
    anchors: set[str] = field(default_factory=set)
    toc: list[tuple[str, str]] = field(default_factory=list)


def split_readme(text: str) -> tuple[str, str]:
    """README without its `## TODO` section, and that section as its own page."""
    m = re.search(r"^## TODO\n", text, re.M)
    if not m:
        return text, ""
    end = re.search(r"^## ", text[m.end() :], re.M)
    stop = m.end() + end.start() if end else len(text)
    todo = "# Roadmap\n" + text[m.end() : stop]
    return text[: m.start()] + text[stop:], todo


def code_block(code: str, lang: str, _attrs: str) -> str:
    try:
        lexer = get_lexer_by_name(lang) if lang else None
    except ClassNotFound:
        lexer = None
    body = highlight(code, lexer, HtmlFormatter(nowrap=True)) if lexer else html.escape(code)
    return f'<pre class="code"><code>{body}</code></pre>'


def renderer() -> MarkdownIt:
    md = MarkdownIt("commonmark", {"html": True, "highlight": code_block})
    md.enable(["table", "strikethrough"])
    md.use(anchors_plugin, min_level=2, max_level=3, permalink=True, permalinkSymbol="#")
    md.use(tasklists_plugin)
    return md


def rewrite_links(page: Page, pages: dict[str, Page], by_anchor: dict[str, str], assets: set[str]) -> None:
    """Point repo-relative links at the right page, asset or GitHub URL."""
    base = posixpath.dirname(page.source)
    by_source = {p.source: p.slug for p in pages.values()}

    def fix(m: re.Match[str]) -> str:
        attr, url = m.group(1), html.unescape(m.group(2))
        if re.match(r"^[a-z]+:", url) or url.startswith("//"):
            return m.group(0)
        if url.startswith("#"):
            anchor = url[1:]
            if anchor not in page.anchors and anchor in by_anchor:
                url = f"{by_anchor[anchor]}#{anchor}"
            return f'{attr}="{html.escape(url)}"'
        path, _, frag = url.partition("#")
        target = posixpath.normpath(posixpath.join(base, path))
        if target in by_source:
            new = by_source[target]
        elif target.startswith("docs/screenshots/"):
            new = "screenshots/" + posixpath.basename(target)
            assets.add(target)
        elif path.endswith("/") or (ROOT / target).is_dir():
            new = TREE + target.rstrip("/")
        else:
            new = BLOB + target
        if frag:
            new += "#" + frag
        return f'{attr}="{html.escape(new)}"'

    page.html = re.sub(r'\b(href|src)="([^"]*)"', fix, page.html)


def tidy(fragment: str) -> str:
    # Tables scroll on their own instead of widening the page.
    fragment = re.sub(r"<table>", '<div class="table"><table>', fragment)
    fragment = re.sub(r"</table>", "</table></div>", fragment)
    # The README's screenshot grid is a table with an empty header row.
    fragment = re.sub(r"<thead>\s*<tr>(\s*<th></th>)+\s*</tr>\s*</thead>", "", fragment)
    # Images load lazily (all but the first, the hero).
    first = True

    def lazy(m: re.Match[str]) -> str:
        nonlocal first
        if first:
            first = False
            return m.group(0)
        return m.group(0).replace("<img ", '<img loading="lazy" ')

    return re.sub(r"<img [^>]*>", lazy, fragment)


def render(md: MarkdownIt, page: Page) -> None:
    tokens = md.parse(page.markdown)
    headings: list[tuple[str, str, str]] = []
    for i, tok in enumerate(tokens):
        if tok.type != "heading_open":
            continue
        text = tokens[i + 1].content
        if tok.tag == "h1" and not page.title:
            page.title = text
        anchor = tok.attrGet("id")
        if anchor:
            page.anchors.add(str(anchor))
            headings.append((tok.tag, str(anchor), text))
    # Contents: the h2s, or the h3s on a page without any (the roadmap).
    level = "h2" if any(tag == "h2" for tag, _, _ in headings) else "h3"
    page.toc = [(a, t) for tag, a, t in headings if tag == level]
    page.html = tidy(md.renderer.render(tokens, md.options, {}))


def inline_md(text: str) -> str:
    return MarkdownIt("commonmark").renderInline(text)


def layout(page: Page, pages: list[Page]) -> str:
    current = ' aria-current="page"'
    nav = "\n".join(f'<a href="{p.slug}"{current if p is page else ""}>{p.nav}</a>' for p in pages)
    toc = "\n".join(f'<li><a href="#{a}">{inline_md(t)}</a></li>' for a, t in page.toc)
    aside = f'<aside class="toc"><p class="label">On this page</p><ul>{toc}</ul></aside>' if toc else ""
    title = "MISO Terminal" if page.slug == "index.html" else f"{page.title} · MISO Terminal"
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title>
<meta name="description" content="A Bloomberg-style information terminal for MISO market data, in Rust. Windows-first, read-only.">
<link rel="icon" href="icon.png">
<link rel="stylesheet" href="style.css">
</head>
<body>
<header class="bar">
  <a class="brand" href="index.html"><img src="icon.png" alt="" width="22" height="22">MISO Terminal</a>
  <nav>{nav}</nav>
  <a class="gh" href="{REPO}">GitHub</a>
</header>
<div class="frame{"" if toc else " single"}">
  {aside}
  <main class="doc">
{page.html}
  </main>
</div>
<footer>
  <p>MISO Terminal is free software under the
  <a href="{BLOB}LICENSE">GNU AGPL v3 or later</a>.
  Not an official MISO product; for information only.
  This site is generated from <a href="{BLOB}{page.source}">{page.source}</a>.</p>
</footer>
</body>
</html>
"""


def theme_cards(files: list[Path], downloadable: bool) -> str:
    """A card per theme file: preview, name, description, palette and download."""
    cards = []
    for f in files:
        t = tomllib.loads(f.read_text(encoding="utf-8"))
        meta, pal = t["meta"], t["palette"]
        tid = meta["id"]
        img = ""
        if (ROOT / f"docs/screenshots/themes/{tid}.webp").exists():
            img = (
                f'<img src="screenshots/themes/{tid}.webp" alt="MISO Terminal in {html.escape(meta["name"])}" '
                f'width="960" height="576" loading="lazy">'
            )
        else:
            print(f"warning: no preview for {tid} (docs/screenshots/themes/{tid}.webp)")
        slots = ["background", "surface", "surface_alt", "text", "text_muted", "accent",
                 "positive", "negative", "warning", "info"]
        colors = [(k, pal[k]) for k in slots] + [(f"series {i}", c) for i, c in enumerate(t["chart"]["series"])]
        swatches = "".join(
            f'<span style="background:{html.escape(c)}" title="{k} {html.escape(c)}"></span>' for k, c in colors
        )
        action = (
            f'<a class="download" href="themes/{tid}.toml" download>Download {tid}.toml</a>'
            if downloadable
            else '<span class="badge">Built in</span>'
        )
        cards.append(f"""<article class="theme-card" id="theme-{tid}">
{img}
<div class="theme-body">
<p class="theme-name">{html.escape(meta["name"])} <code>{tid}</code></p>
<p class="theme-desc">{html.escape(meta.get("description", ""))}</p>
<div class="swatches">{swatches}</div>
{action}
</div>
</article>""")
    return '<div class="theme-grid">\n' + "\n".join(cards) + "\n</div>"


def builtin_theme_files() -> list[Path]:
    # The default first, then by name.
    return sorted((ROOT / "themes").glob("*.toml"), key=lambda f: (f.stem != "default", f.stem))


def gallery_theme_files() -> list[Path]:
    return sorted((ROOT / "themes/gallery").glob("*.toml"))


def gallery_index() -> str:
    """The index the terminal's THEME gallery reads (mt_theme::gallery): every
    gallery theme's file verbatim, so one request lists and installs them."""
    themes = []
    for f in gallery_theme_files():
        src = f.read_text(encoding="utf-8")
        meta = tomllib.loads(src)["meta"]
        themes.append({
            "id": meta["id"],
            "name": meta["name"],
            "dark": meta.get("dark", True),
            "file": f"themes/{f.name}",
            "preview": f"screenshots/themes/{meta['id']}.webp",
            "toml": src,
        })
    return json.dumps({"version": 1, "themes": themes}, ensure_ascii=False, indent=1) + "\n"


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    update_fixture = "--update-fixture" in sys.argv[1:]
    out = Path(args[0] if args else "site").resolve()
    readme, roadmap = split_readme((ROOT / "README.md").read_text(encoding="utf-8"))
    pages = [
        Page("index.html", "README.md", "Overview", readme),
        Page("architecture.html", "docs/ARCHITECTURE.md", "Architecture"),
        Page("extending.html", "docs/EXTENDING.md", "Extending"),
        Page("themes.html", "themes/README.md", "Themes"),
        Page("roadmap.html", "README.md#todo", "Roadmap", roadmap),
        Page("markets-plan.html", "docs/MARKETS-PLAN.md", "Markets plan"),
        Page("releases.html", "docs/RELEASES.md", "Release plan"),
    ]
    md = renderer()
    for p in pages:
        if not p.markdown:
            p.markdown = (ROOT / p.source).read_text(encoding="utf-8")
        render(md, p)
    by_slug = {p.slug: p for p in pages}
    by_anchor: dict[str, str] = {}
    for p in pages:
        for a in p.anchors:
            by_anchor.setdefault(a, p.slug)
    assets: set[str] = set()
    for p in pages:
        rewrite_links(p, by_slug, by_anchor, assets)
    themes_page = by_slug["themes.html"]
    for marker, cards in [
        ("<!-- builtin-cards -->", theme_cards(builtin_theme_files(), downloadable=False)),
        ("<!-- gallery-cards -->", theme_cards(gallery_theme_files(), downloadable=True)),
    ]:
        assert marker in themes_page.html, f"themes/README.md lost its {marker} marker"
        themes_page.html = themes_page.html.replace(marker, cards)

    if out.exists():
        # Only ever replace an empty directory or an earlier (even half-finished) build.
        if any(out.iterdir()) and not (out / MARKER).exists():
            sys.exit(f"refusing to replace {out}: not empty and not an earlier build of this site")
        shutil.rmtree(out)
    (out / "screenshots").mkdir(parents=True)
    (out / MARKER).write_text("Built by tools/build_docs.py; replaced on every build.\n", encoding="utf-8", newline="\n")
    (out / "fonts").mkdir()
    (out / "themes").mkdir()
    (out / "screenshots/themes").mkdir()
    for p in pages:
        (out / p.slug).write_text(layout(p, pages), encoding="utf-8", newline="\n")
    for a in sorted(assets):
        shutil.copy2(ROOT / a, out / "screenshots" / posixpath.basename(a))
    for f in gallery_theme_files():
        shutil.copy2(f, out / "themes" / f.name)
    index = gallery_index()
    (out / "themes/index.json").write_text(index, encoding="utf-8", newline="\n")
    if update_fixture:
        FIXTURE.parent.mkdir(parents=True, exist_ok=True)
        FIXTURE.write_text(index, encoding="utf-8", newline="\n")
        print(f"updated {FIXTURE.relative_to(ROOT)}")
    for f in (ROOT / "docs/screenshots/themes").glob("*.webp"):
        shutil.copy2(f, out / "screenshots/themes" / f.name)
    for f in (ROOT / "assets/fonts").glob("*-Variable.ttf"):
        shutil.copy2(f, out / "fonts" / f.name)
    shutil.copy2(ROOT / "assets/icon/miso-terminal.png", out / "icon.png")
    shutil.copy2(Path(__file__).with_name("docs_style.css"), out / "style.css")
    print(f"{len(pages)} pages, {len(assets)} screenshots, {len(gallery_theme_files())} gallery themes -> {out}")


if __name__ == "__main__":
    main()
