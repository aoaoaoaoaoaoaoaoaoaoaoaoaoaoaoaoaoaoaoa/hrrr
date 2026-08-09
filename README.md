# HRRR

HRRR is a native viewer for NOAA High-Resolution Rapid Refresh forecast
fields. It renders surface smoke, two-metre temperature, total cloud cover,
total precipitation, and one-hour precipitation over a locally indexed vector
basemap. Forecast animation, map navigation, probes, and saved views remain
responsive while network, GRIB, PMTiles, and mesh work run outside the UI
thread.

## Install

HRRR requires Rust 1.96 or newer and a working wgpu-compatible graphics stack.
The released native hosts are Linux/X11, macOS, and Windows.

```sh
cargo install hrrr --locked
hrrr basemap install
hrrr
```

`basemap install` is explicit because it downloads several GiB. It obtains a
pinned, SHA-256-verified `go-pmtiles` binary for the current platform, extracts
the North American portion of the current [Protomaps daily
build](https://maps.protomaps.com/), verifies the resulting PMTiles archive,
and discards the extraction tool. The z12 archive is currently about 2.3 GiB.
Pass a historical build date as `hrrr basemap install YYYYMMDD`.

Inspect or remove the basemap with:

```sh
hrrr basemap status
hrrr basemap remove
```

Linux uses an XEmbed tray; lack of a tray degrades window-close behavior to
ordinary termination. macOS and Windows use their native tray facilities.
Linux/Wayland is not yet a release coordinate.

## Use

Click an active field button again to show only the basemap. The arrow keys,
forecast rail, `Ctrl+R`, and `Ctrl+Shift+R` select forecast time, the latest
run, and the latest 48-hour run.

Drag to pan and scroll at the pointer to zoom. A left click moves the transient
probe; `Shift`-left-click creates a persistent probe. Drag a persistent probe
by its bulb and remove it with its adjacent ×. `Esc` clears the transient
probe. `Ctrl+Z` undoes probe placement, movement, removal, and transient
clearing. Map navigation and saved-view operations remain outside that history.

Every map position and persistent-probe set belongs to the active saved view
and is autosaved. The + control clones the active view. Numeric keys select
bound views; `Shift` plus a numeric key binds that slot to the active view.

Closing the window hides it only when **Close minimizes** is enabled and a tray
is available. Left-click the tray icon to reveal the window; its context menu
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

Forecast cache entries expire after seven days and the cache is capped at
512 MiB. `cargo uninstall hrrr` removes the executable but preserves user
preferences, views, and the explicitly installed basemap. Use `hrrr basemap
remove` before uninstalling when that data should also be removed.

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
field selection and restart restoration; transient and persistent probes; pin
drag and undo; and tray hide, reveal, menu, and quit behavior. Failure evidence
is retained under `/tmp/hrrr-acceptance-artifacts` by default.

The portability controller launches the real witnessed product on macOS arm64,
macOS x86_64, and Windows x86_64, then requires CLI identity, native tray
construction, a successfully presented GPU frame, the current HRRR witness
contract, and the map surface. Each host also runs the product laws, verifies
the crates.io package graph, and proves ordinary install/uninstall behavior.

The UI vocabulary is a separately published dependency. When it changes,
release it first with `scripts/release-contract VERSION publish`; only after
that exact version is visible on crates.io may `scripts/release VERSION
publish` seal the application. Both release commands require a clean, pushed
`main` checkout and a valid signed tag at `HEAD`.

## License

HRRR is distributed under the [MIT License](LICENSE).
