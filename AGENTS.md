hrrr: native viewer for NOAA HRRR forecast fields

## gate
- `./check.py check` after edits; `./check.py verify` for non-mutating gate

## invariants
- Network and GRIB work never on UI thread
- Forecast products, units, palettes, runs, lead times: closed domain types
- User-authored config in XDG config; disposable forecast bytes in XDG cache; ephemeral view state in XDG state
- New UI mechanisms stay local until polished enough to promote into `brass_poolrooms`
