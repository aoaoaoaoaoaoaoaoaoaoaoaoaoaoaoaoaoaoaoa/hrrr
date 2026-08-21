# HRRR

HRRR is a native viewer for NOAA High-Resolution Rapid Refresh forecast
fields. It renders surface smoke, two-metre temperature, ten-metre wind, total
cloud cover, total precipitation, and one-hour precipitation over a locally
indexed vector basemap. Standard wind barbs are stamped from application-owned
3D bronze dies by the same build-time Foundry that makes Poolrooms chrome.
Forecast animation, map navigation, probes, and saved views remain responsive
while network, GRIB, PMTiles, and mesh work run outside the UI thread.

## Install

HRRR requires a working wgpu-compatible graphics stack. Releases support Linux
on X11 and Wayland, macOS 13 or newer on Apple Silicon and Intel, and 64-bit
Windows.

On Linux, install the ordinary command-line package with Rust 1.96 or newer:

```sh
cargo install hrrr --locked
hrrr
```

macOS uses the universal
[DMG](https://github.com/aoaoaoaoaoaoaoaoaoaoaoaoaoaoaoaoaoaoaoa/hrrr/releases/latest/download/hrrr-macos-universal.dmg).
Drag HRRR into Applications. The image is not yet signed or notarized; macOS
therefore requires the standard manual override for an unidentified developer.

Windows uses the per-user
[installer](https://github.com/aoaoaoaoaoaoaoaoaoaoaoaoaoaoaoaoaoaoaoa/hrrr/releases/latest/download/hrrr-windows-x86_64-setup.exe).
It needs no administrator access and creates the normal Start Menu entry. The
binary is not yet code-signed, so SmartScreen may require explicit approval.

First launch explains and offers the basemap installation. Approval downloads
about 1.1 GB and leaves a roughly 1.01 GiB North American map core available
offline through zoom 11. HRRR obtains a pinned, SHA-256-verified `go-pmtiles`
binary for the current platform, extracts the current [Protomaps daily
build](https://maps.protomaps.com/), verifies the result, and discards the tool.
At zoom 12, only visible detail tiles are fetched by HTTP range request. Those
tiles enter a 512 MiB, seven-day disposable cache; zoom 11 remains visible when
the detail source is unavailable.

The same operation remains available explicitly. A date pins both the local
core and its lazy detail tier to a historical daily build:

```sh
hrrr basemap install [YYYYMMDD]
```

Inspect or remove the basemap with:

```sh
hrrr basemap status
hrrr basemap remove
```

Linux/X11 uses an XEmbed tray. Pure Wayland has no tray integration, so
window-close behavior degrades to ordinary termination. macOS and Windows use
their native tray facilities.

## Use

Click an active field button again to show only the basemap. Focus a forecast
rail to adjust it with the arrow keys; a hovered rail also accepts the mouse
wheel. `Ctrl+R` and `Ctrl+Shift+R` select the latest run and latest 48-hour run.
Cumulative fields add a **Base hour** rail; their map shows the increment from
that hour through the selected forecast hour.

Press `F1` or `?` for the generated command guide. `Tab` and `Shift+Tab` move
within the active inspector panel; `Ctrl+Tab` and `Ctrl+Shift+Tab` cross panels.
Permanently underlined letters mark conservative `Alt` mnemonics. The guide
owns keyboard input while open, and `Esc` closes only its topmost layer.

Drag to pan and scroll at the pointer to zoom. The map scale appears at lower
left; maximum zoom is roughly two kilometres across a full-HD viewport. A left
click moves the transient probe; `Shift`-left-click creates a persistent probe.
Drag a persistent probe by its bulb and remove it with its adjacent ×. `Esc`
clears the transient probe. `Ctrl+Z` undoes probe placement, movement, removal,
and transient clearing. Map navigation and saved-view operations remain outside
that history.

Every map position and persistent-probe set belongs to the active saved view
and is autosaved. The + control clones the active view. Numeric keys select
bound views; `Shift` plus a numeric key binds that slot to the active view.
Autosave settlement is a semantic deadline rather than a repaint clock, and
durable writes run on the shared Eternalist background scribe.
Forecast-frontier surveys and basemap retry backoff use that same host service
clock, so rendering can stop completely without suspending domain time.

Closing the window hides it only when **Close to tray** is enabled and both the
native window system and tray support concealment. Otherwise close terminates
the process. Left-click the tray icon to reveal the window; its context menu
quits the process.

## Storage

HRRR follows the host platform’s application-directory conventions. The
platform roots are:

| Host | Persistent root | Disposable root |
| --- | --- | --- |
| Linux | XDG config, data, and state roots under `hrrr/` | `$XDG_CACHE_HOME/hrrr/` |
| macOS | `~/Library/Application Support/moe.swarm.hrrr/` | `~/Library/Caches/moe.swarm.hrrr/` |
| Windows | `%APPDATA%\swarm\hrrr\` | `%LOCALAPPDATA%\swarm\hrrr\cache\` |

On Linux the individual defaults are:

| Meaning | Path |
| --- | --- |
| preferences | `$XDG_CONFIG_HOME/hrrr/config.toml` |
| saved views and basemap | `$XDG_DATA_HOME/hrrr/` |
| session state | `$XDG_STATE_HOME/hrrr/slate.toml` |
| disposable forecasts | `$XDG_CACHE_HOME/hrrr/fields/` |

Unset roots use the XDG defaults. Relative XDG roots are ignored. Set
`HRRR_BASEMAP_ARCHIVE` to an absolute PMTiles path to use an externally managed
archive.

Forecast and detailed-basemap caches each expire after seven days and are each
capped at 512 MiB. Cargo, the macOS app bundle, and the Windows uninstaller
remove installed machinery while preserving preferences, views, and the
explicitly installed basemap. Run `hrrr basemap remove` first to remove the
local core and cached detail as well.

## Data

Forecast data is fetched directly from the [NOAA HRRR public
archive](https://registry.opendata.aws/noaa-hrrr-pds/). Basemap data comes from
[OpenStreetMap](https://www.openstreetmap.org/copyright) through Protomaps.
Displayed forecasts are model output, not official warnings or observations.

## Development

Run the non-mutating source gate with `./check.py verify`. `scripts/test-gui`
builds the optimized product twice, first ordinarily and then with its one-way
test witness, and runs the complete Linux suite in private X11, XDG, process,
network, and software-graphics namespaces. The stories prove inert launch;
generated-help presentation and keyboard containment; panel traversal; field
selection and restart restoration; transient and persistent probes; pin drag
and undo; and tray hide, reveal, menu, and quit behavior. Failure evidence is
retained under `/tmp/hrrr-acceptance-artifacts` by default.

The Foundry contract runs one native runtime proof on Linux/X11,
Linux/Wayland, macOS arm64, macOS x86_64, and Windows x86_64. The Wayland cell
owns a headless Weston compositor and requires both a witnessed surface present
and captured nonblack output; it makes no native-input parity claim. X11 keeps
the full native interaction suite. The macOS and Windows controllers require
CLI identity, a successfully presented GPU frame, the current HRRR witness
contract, first-contact basemap consent, and the map surface. Installer cells
also forge a universal DMG and an NSIS package, install or mount the exact
artifact, rerun the controller against the packaged executable, and prove
uninstallation without user-data loss.

The UI vocabulary is a separately versioned dependency.
`scripts/release-contract VERSION publish` can release it independently;
`scripts/release VERSION publish` proves and publishes any missing contract
version, waits for the registry boundary, verifies the isolated application
tarball against that exact dependency, then publishes the application. Both
release commands require a clean, pushed `main` checkout and a valid signed tag
at `HEAD`. Registry publication remains deliberate, but a version tag cannot
publish the installers until that exact crate version is visible. Foundry must
also judge the complete source, security, package, host, lifecycle,
native-acceptance, and artifact evidence graph.

## License

HRRR is distributed under the [MIT License](LICENSE).
