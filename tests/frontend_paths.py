"""Select an already-built frontend and inspect its actual HTML resources."""
import os
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit


def frontend_dir():
    repository = Path(__file__).resolve().parents[1]
    return Path(os.environ.get("SESSIONDOCK_TEST_WEB_DIR") or repository / "legacy-web").resolve()


class FrontendHTML(HTMLParser):
    def __init__(self, source):
        super().__init__()
        self.modules = []
        self.classic = []
        self.styles = []
        self.resources = []
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if tag == "script" and values.get("src"):
            target = self.modules if values.get("type") == "module" else self.classic
            target.append(values["src"])
            self.resources.append(values["src"])
        if tag == "link" and values.get("href"):
            rel = set(values.get("rel", "").split())
            if "stylesheet" in rel:
                self.styles.append(values["href"])
            if rel & {"stylesheet", "preload", "modulepreload", "manifest", "icon", "apple-touch-icon"}:
                self.resources.append(values["href"])


def local_asset(directory: Path, src: str, page: str = "index.html") -> Path:
    """Resolve a local HTML resource, refusing missing/outside artifact paths."""
    directory = Path(directory).resolve()
    url = urlsplit(src)
    assert not url.scheme and not url.netloc, f"external resource: {src}"
    path = unquote(url.path)
    entry = ((directory if path.startswith("/") else (directory / page).parent)
             / path.lstrip("/")).resolve()
    assert entry.is_relative_to(directory) and entry.is_file(), f"missing local resource in {directory}: {src}"
    return entry


def frontend_html(directory: Path | None = None, page: str = "index.html") -> FrontendHTML:
    directory = Path(directory or frontend_dir())
    return FrontendHTML((directory / page).read_text(encoding="utf-8"))


def entry_asset(directory: Path | None = None, page: str = "index.html") -> Path:
    """Find the actual module or historical classic entry in an artifact."""
    directory = Path(directory or frontend_dir()).resolve()
    parser = frontend_html(directory, page)
    classic_entry = "app.js" if page == "index.html" else "file.js" if page == "files.html" else Path(page).with_suffix(".js").name
    candidates = parser.modules or [src for src in parser.classic
                                   if Path(urlsplit(src).path).name == classic_entry]
    assert len(candidates) == 1, f"{page}: expected one entry, found {candidates}"
    return local_asset(directory, candidates[0], page)


def html_assets(directory: Path, page: str = "index.html") -> list[Path]:
    """List local resources declared by this page, including its entry module."""
    return list(dict.fromkeys(local_asset(directory, src, page)
                              for src in frontend_html(directory, page).resources
                              if not urlsplit(src).scheme and not urlsplit(src).netloc))
