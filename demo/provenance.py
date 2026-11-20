#!/usr/bin/env python3
"""Fetch a published file, and remember where and when it came from.

An index records each source it was derived from by content hash, and may
record where it came from and when (`SPEC.md` §6). The hash can be taken from
the file at build time; the other two cannot. Where is lost when a file is
downloaded, and when is lost the moment the publisher replaces it. So this
writes them down at the moment they are known, next to the file:

    python3 provenance.py fetch <url> <path>

downloads `<url>` to `<path>` and writes `<path>.source`, a JSON record of the
URL, the publisher's `Last-Modified` if it sent one, and the time of the
download.

    python3 provenance.py fetch --archive-date <url> <path>

does the same, and when the server sends no `Last-Modified`, takes the date of
the newest file inside the zip archive instead — the publisher's own tooling
wrote it, so it is the publisher's time, but it is a weaker statement than a
header, and the record says where it came from (`modified_from`). Zip times
carry no time zone, so only the date is kept. Then

    mokuji build corpus.tsv index.mokuji \\
        $(python3 provenance.py args <path>...)

turns each file and its record into the flags `mokuji build` takes:

    --source <path> --source-uri <url>
    --source-modified <time> --source-retrieved <time>

A file without a record is passed as a bare `--source`, so a script written by
hand still counts.

The hash recorded is of the file as it was served — the archive, not what was
unpacked from it — so anyone can fetch the URL in the index and check the hash.
"""

import email.utils
import json
import pathlib
import sys
import urllib.request
import zipfile
from datetime import date, datetime, timezone


def rfc3339(moment):
    return moment.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def archive_date(path):
    """The date of the newest entry in a zip archive, or None."""
    try:
        with zipfile.ZipFile(path) as archive:
            stamps = [info.date_time for info in archive.infolist()]
    except zipfile.BadZipFile:
        return None
    if not stamps:
        return None
    return date(*max(stamps)[:3]).isoformat()


def fetch(url, path, use_archive_date=False):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    request = urllib.request.Request(url, headers={"User-Agent": "mokuji-demo"})
    with urllib.request.urlopen(request) as response:
        partial = path.with_name(path.name + ".part")
        with partial.open("wb") as handle:
            while chunk := response.read(1 << 20):
                handle.write(chunk)
        last_modified = response.headers.get("Last-Modified")
        final_url = response.geturl()
    retrieved = rfc3339(datetime.now(timezone.utc))
    partial.replace(path)

    record = {"uri": url, "retrieved": retrieved}
    if last_modified:
        try:
            record["modified"] = rfc3339(email.utils.parsedate_to_datetime(last_modified))
        except (TypeError, ValueError):
            print(f"{url}: unreadable Last-Modified {last_modified!r}", file=sys.stderr)
    if "modified" not in record and use_archive_date:
        if stamp := archive_date(path):
            record["modified"] = stamp
            record["modified_from"] = "the newest entry in the archive"
    # A redirect is worth knowing about, but the URL asked for is the one that
    # names the dataset; where it happened to be served from today is not.
    if final_url != url:
        record["served_from"] = final_url
    pathlib.Path(f"{path}.source").write_text(
        json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"{path}  modified {record.get('modified', 'unknown')}", file=sys.stderr)


def args(paths):
    flags = []
    for path in paths:
        flags += ["--source", path]
        sidecar = pathlib.Path(f"{path}.source")
        if not sidecar.exists():
            continue
        record = json.loads(sidecar.read_text(encoding="utf-8"))
        for key, flag in (
            ("uri", "--source-uri"),
            ("modified", "--source-modified"),
            ("retrieved", "--source-retrieved"),
        ):
            if record.get(key):
                flags += [flag, record[key]]
    for flag in flags:
        # Printed for $(...), which splits on whitespace; none of these
        # values has any, and one that did would be split silently.
        if any(c.isspace() for c in flag):
            sys.exit(f"cannot pass {flag!r} through $(...): it contains whitespace")
    print(" ".join(flags))


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "fetch":
        fetch(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 5 and sys.argv[1:3] == ["fetch", "--archive-date"]:
        fetch(sys.argv[3], sys.argv[4], use_archive_date=True)
    elif len(sys.argv) >= 3 and sys.argv[1] == "args":
        args(sys.argv[2:])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
