# GitHub Pages

The site is live at <https://monstercameron.github.io/CounterSplat/> and the
browser demo at <https://monstercameron.github.io/CounterSplat/game/>.

The landing page lives in `docs/index.html`; `docs/game/` is its game launcher.
The workflow (`.github/workflows/pages.yml`) builds a static site and publishes
it through GitHub Pages on every push to `main`, and on demand.

## The demo level streams from its publisher

The demo plays **The Barn** by dongiku ([superspl.at/scene/58d333ae](https://superspl.at/scene/58d333ae)).
Scene authors keep their rights, so the site does not redistribute the scan:
its `docs/pages-assets.json` entry sets `hosted` to the publisher's own content
URL, the generated catalog carries it, and the game streams the splats and
collision voxels from there (the CDN answers with `Access-Control-Allow-Origin: *`).
Only the level's start camera (`viewer-settings.json`) and our provenance record
(`source.json`) ship with the site. The catalog validates `hosted` in Rust
(`LevelCatalog`: an `https://` base ending in `/`, no credentials or query), and
the page credits the level and links to it.

The gameplay scene's other media ship too: the CC0 HDR sky, silent placeholder
sounds and a 1x1 placeholder first footage frame (see the `ASSET-NOTICE.md`
files beside them). The weapon footage is being remade, so the scene spawns with
the weapon stowed (`WeaponConfig.start_stowed`; F raises it), and the build
leaves out the on-demand (`streamable`) frames that are not available, printing
how many.

## Review assets before publishing

`docs/pages-assets.json` selects what is published; it now lists The Barn as a
hosted level (above). With no levels selected, the published artifact contains
the landing page and an honest "being prepared" game page. Only selected,
tracked files are copied; no local scans, unselected media, credentials, or
build caches are.

Inspect available local media without staging or publishing it:

```powershell
python scripts/build_pages.py --inventory
```

A level whose files you may redistribute can instead be copied into the site
by tracking them and adding an entry without `hosted`, for example:

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

## Publishing

The repository's Pages source is **GitHub Actions** (the workflow's
`configure-pages` step enables it). Each push to `main` runs the **GitHub
Pages** workflow, which builds WebAssembly when a level is selected, assembles
the site, uploads the Pages artifact and deploys it; run it by hand from the
Actions tab to redeploy without a push. The site and game use relative URLs and
support project-site subpaths and custom domains. Nothing is deployed by the
local build.
