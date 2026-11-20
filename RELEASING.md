# Releasing

A `v*` tag publishes everything at once: the command to GitHub Releases and
Homebrew (`.github/workflows/release.yml`), and the crate, the npm package and
the Python package (`.github/workflows/publish.yml`).

## A release

1. Set one version in `Cargo.toml`, `js/package.json` and
   `python/pyproject.toml`. Both workflows refuse a tag that any of them
   disagrees with.
2. Let CI pass on that commit.
3. Tag it and push the tag:

   ```sh
   git tag v0.1.0 && git push origin v0.1.0
   ```

## Before the first

Each registry is told once, by someone who owns the name there, to trust
`publish.yml`. Nothing else is configured on GitHub for them: the workflow
holds no registry token.

| | Where | What to enter |
|---|---|---|
| crates.io | The crate's settings, *Trusted Publishing* | Repository `reearth/mokuji`, workflow `publish.yml`, environment `crates-io` |
| npm | The package's settings, *Trusted Publisher* | Organization or user `reearth`, repository `mokuji`, workflow `publish.yml`, environment `npm` |
| PyPI | *Your projects* → *Publishing*, a pending publisher | Project `mokuji`, owner `reearth`, repository `mokuji`, workflow `publish.yml`, environment `pypi` |

Two of them need more than that:

- **crates.io** can only trust a workflow for a crate that exists, so the
  first version is published by hand, with an API token, from a clean checkout
  of the tagged commit: `cargo publish`.
- **npm**: `js/package.json` is `"private": true` until publishing is
  decided, and the workflow stops on it. Remove it in the commit that is
  tagged. A trusted publisher is likewise set on a package that exists, so
  the first version is `npm publish --access public` by hand, from `js/`.

PyPI accepts a pending publisher for a name that does not exist yet, so its
first version comes from the workflow.

Homebrew needs the repository to be public, since a cask downloads the
release's archives; the tap, `reearth/homebrew-tap`, is written through the
org's reearth-app, which `release.yml` is given in the `release` environment.
