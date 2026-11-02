# GitHub Pages

The landing page lives in `docs/index.html`; `docs/game/` is its game launcher.
The workflow builds a static site and publishes it through GitHub Pages. It is
manual-only: creating these files does not publish the site.

## Review assets before publishing

`docs/pages-assets.json` starts with an empty `levels` list. With no levels
selected, the published artifact contains the landing page and an honest
"being prepared" game page. No local scans, weapon footage, audio, HDR files,
credentials, or build caches are copied into that artifact.

Inspect available local media without staging or publishing it:

```powershell
python scripts/build_pages.py --inventory
```

When you decide what to publish, add an entry for each chosen level, for example:

```json
{
  "version": 1,
  "levels": [
    {
      "id": "58d333ae",
      "name": "The Barn",
      "creator": "dongiku",
      "format": "single_sog",
      "collision": true,
      "thumbnail": true
    }
  ]
}
```

Each level is opt-in. Set `collision` and `thumbnail` to `true` only if you
want those optional files included; omitted values default to `false`. For a
streamed level use `streamed_sog`; its manifest and all referenced LOD files
must be available. The builder generates the website's catalog from this
selection and copies only referenced level data, optional selected files, and
the scene-declared game resources.

The current gameplay scene declares a shared HDR sky, weapon frames, and
sounds; all relative asset and script URIs in `gameplay.json` and `menu.json`
must be available for a playable build. The normal Pages build requires the
selected assets to be tracked by Git and fails if any required file is missing
or untracked. Gitignore rules are unchanged; add only the media you choose
after reviewing its size and redistribution rights. The build never reads
`.env` or key files.

GitHub Pages' published site limit is 1 GB. The build checks this limit; see
[GitHub Pages limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits).
Large splat levels may require choosing a smaller level or using another asset
host in a future integration.

## Preview locally

The landing page without game media can be assembled with:

```powershell
python scripts/build_pages.py
python -m http.server 18764 --bind 127.0.0.1 --directory .artifacts
```

Open `http://127.0.0.1:18764/pages/`. To test a locally available, ignored
level without changing the selection file or committing its media, first build
the browser package, then run:

```powershell
pwsh -NoProfile -File scripts/build_web.ps1 -SkipSceneAssets
python scripts/build_pages.py --output .artifacts/pages-local --preview-local 58d333ae
```

Open `http://127.0.0.1:18764/pages-local/`. The output is ignored under
`.artifacts/`. The local preview flag is not used by the publishing workflow.

## Publish later

After reviewing and committing the site code, asset selection, and chosen
tracked media, configure the repository's Pages source as **GitHub Actions**
and run the **GitHub Pages** workflow manually. The workflow builds WebAssembly
only when a level is selected, assembles the site, uploads the Pages artifact,
and deploys it. The site and game use relative URLs and support project-site
subpaths and custom domains. Nothing is deployed by the local build.
