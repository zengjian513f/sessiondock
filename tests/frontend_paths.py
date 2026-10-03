"""Select the static frontend for private browser services."""
import os
from pathlib import Path


def frontend_dir():
    repository = Path(__file__).resolve().parents[1]
    return Path(os.environ.get("SESSIONDOCK_TEST_WEB_DIR", repository / "legacy-web")).resolve()
