# What the tests share: where the conformance suite is, and a fixture held in
# memory, so the reader can be run without a file or a server.

import sys
import tomllib
from pathlib import Path

# The package beside these tests, so that they run from a checkout without
# installing it: `python3 -m unittest discover -s python/tests`.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

# A fixture held in memory is read through the reader's own source, as any
# caller's would be.
from mokuji import BytesSource  # noqa: E402

CONFORMANCE = Path(__file__).resolve().parents[2] / "conformance"
FIXTURES = CONFORMANCE / "fixtures"
REPOSITORY = CONFORMANCE.parent


def fixture_bytes(name):
    return (FIXTURES / f"{name}.mokuji").read_bytes()


def fixture_source(name):
    return BytesSource(fixture_bytes(name))


def cases():
    # Read rather than restated: a copy of the expectations would drift, and
    # the whole point is that every implementation answers to the same ones.
    with open(CONFORMANCE / "cases.toml", "rb") as f:
        return tomllib.load(f)
