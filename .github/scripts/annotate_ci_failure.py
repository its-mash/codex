#!/usr/bin/env python3
"""Publish the useful tail of a CI log as a GitHub check annotation."""

import re
import sys
from pathlib import Path


ANSI_ESCAPE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
MAX_ANNOTATION_CHARS = 16_000


def escape_workflow_command(value: str) -> str:
    return value.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def escape_workflow_property(value: str) -> str:
    return escape_workflow_command(value).replace(":", "%3A").replace(",", "%2C")


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} TITLE LOG_PATH")
    title, log_path = sys.argv[1:]
    log = Path(log_path).read_text(encoding="utf-8", errors="replace")
    tail = ANSI_ESCAPE.sub("", log[-MAX_ANNOTATION_CHARS:])
    print(
        f"::error title={escape_workflow_property(title)}::{escape_workflow_command(tail)}"
    )


if __name__ == "__main__":
    main()
