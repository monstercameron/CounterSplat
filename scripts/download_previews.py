#!/usr/bin/env python3
"""Restore public SuperSplat preview images without redownloading level data."""
import argparse
from html.parser import HTMLParser
import re
import sys
from urllib.parse import urlsplit

from download_supersplat import DESTINATION, ORDER, SCENES, cached_valid, run_jobs, small_response


class PreviewMetadata(HTMLParser):
    def __init__(self):
        super().__init__()
        self.url = None

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag.lower() == "meta" and attributes.get("property") == "og:image":
            self.url = attributes.get("content")


def preview_url(scene_id, page):
    """Accept only the preview declared for this exact public scene."""
    parser = PreviewMetadata()
    parser.feed(page)
    parsed = urlsplit(parser.url or "")
    expected = r"/images\.playcanvas\.com/splat/" + re.escape(scene_id) + r"/v[0-9]+/xl\.webp"
    if (parsed.scheme != "https" or parsed.netloc != "s3-eu-west-1.amazonaws.com"
            or not re.fullmatch(expected, parsed.path) or parsed.query or parsed.fragment):
        raise ValueError("missing or unexpected public preview URL")
    return parser.url


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ids", nargs="*", help="scene IDs; default: all configured levels")
    parser.add_argument("--workers", type=int, default=4, help="parallel downloads (1–16)")
    args = parser.parse_args()
    if not 1 <= args.workers <= 16:
        parser.error("workers must be between 1 and 16")
    chosen = list(dict.fromkeys(args.ids or ORDER))
    if any(scene_id not in SCENES for scene_id in chosen):
        parser.error("unknown scene ID")
    jobs, errors = [], []
    for scene_id in chosen:
        destination = DESTINATION / scene_id / "preview.webp"
        if cached_valid(destination):
            continue
        try:
            page = small_response("https://superspl.at/scene/" + scene_id, limit=2 * 1024 * 1024).decode("utf-8")
            jobs.append((scene_id, preview_url(scene_id, page), destination))
        except Exception as error:
            errors.append((scene_id, str(error)))
            print(f"FAILED {scene_id}: {error}", flush=True)
    _, failures = run_jobs("Level previews", jobs, args.workers)
    return int(bool(errors or failures))


if __name__ == "__main__":
    sys.exit(main())
