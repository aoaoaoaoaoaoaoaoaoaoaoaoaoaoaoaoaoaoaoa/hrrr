# HRRR

HRRR is a native viewer for NOAA High-Resolution Rapid Refresh forecast
fields. It renders surface smoke, two-metre temperature, total precipitation,
and one-hour precipitation over a locally indexed vector basemap. Forecast
animation, map navigation, probes, and saved views remain responsive while
network, GRIB, PMTiles, and mesh work run outside the UI thread.

## Install

HRRR requires Rust 1.96 or newer, Linux/X11, and a working Vulkan graphics
stack.

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

Linux/X11 is the sole current native-host coordinate. The tray implementation
uses XEmbed; lack of a tray degrades window-close behavior to ordinary
termination. Wayland, macOS, and Windows require proved Eternalist host
projections before they can be claimed.

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

HRRR follows the host platform’s application-directory conventions. On Linux
the defaults are:

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
test witness, and runs the complete native suite in private X11, XDG, process,
network, and software-graphics namespaces. The stories prove inert launch;
transient and persistent probes; restart restoration; pin drag and undo; and
tray hide, reveal, menu, and quit behavior. Failure evidence is retained under
`/tmp/hrrr-acceptance-artifacts` by default.

Native acceptance and ordinary-product CI share one declared coordinate:
Linux/X11.

## License

HRRR is distributed under the [MIT License](LICENSE).
