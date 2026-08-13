# AGENTS.md

Read `/home/main/programming/projects/rust_starter/docs/rust-style-doctrine.md`
before meaningful Rust edits.

Run `./check.py check` after meaningful edits and `./check.py verify` for the
non-mutating gate.

This is a native forecast-field viewer. Network and GRIB work never runs on the
UI thread. Forecast products, units, palettes, runs, and lead times are closed
domain types. User-authored configuration belongs under XDG config, disposable
forecast bytes under XDG cache, and ephemeral view state under XDG state.

New UI mechanisms remain local until their design is polished enough to
promote into `brass_poolrooms`.
