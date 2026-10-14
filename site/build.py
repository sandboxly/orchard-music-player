"""Build the website into _site/: a copy of site/ with the album art credits
from site/credits.json filled into the page.

    python3 site/build.py    then serve _site/ (python3 -m http.server -d _site)
"""

from __future__ import annotations

import html
import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
OUT = ROOT / "_site"
MARK = "<!-- credits -->"


def link(text: str, url: str) -> str:
    text = html.escape(text)
    return f'<a href="{html.escape(url)}">{text}</a>' if url.startswith(("https://", "http://")) else text


def credits_html() -> str:
    """The "Album art in the screenshots" list, or "" without a usable credits.json."""
    path = SITE / "credits.json"
    try:
        items = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        print("No site/credits.json: building without album art credits")
        return ""
    except (OSError, ValueError) as e:
        print(f"Skipping album art credits: can't read {path.name} ({e})")
        return ""
    rows = []
    for item in items if isinstance(items, list) else []:
        if not isinstance(item, dict) or not item.get("album") or not item.get("artist"):
            continue
        album = link(str(item["album"]), str(item.get("source_url", "")))
        line = f"{album} by {html.escape(str(item['artist']))}"
        if item.get("license"):
            line += " · " + link(str(item["license"]), str(item.get("license_url", "")))
        rows.append(f"        <li>{line}</li>")
    if not rows:
        return ""
    return (
        '<section class="credits wrap" aria-labelledby="credits-title">\n'
        '    <h2 id="credits-title">Album art in the screenshots</h2>\n'
        "    <ul>\n" + "\n".join(rows) + "\n    </ul>\n  </section>"
    )


def build() -> None:
    shutil.rmtree(OUT, ignore_errors=True)
    shutil.copytree(SITE, OUT, ignore=shutil.ignore_patterns("build.py", "credits.json", "__pycache__", ".DS_Store"))
    index = OUT / "index.html"
    index.write_text(index.read_text(encoding="utf-8").replace(MARK, credits_html()), encoding="utf-8")
    print(f"Built {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    build()
    sys.exit(0)
