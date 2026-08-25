use std::{ffi::OsStr, fmt, fs, path::Path, process::Command};

use egui_tester::{Error, Result};
use hrrr_choreography::Score;
use serde::{Deserialize, de::DeserializeOwned};

const OVERALL: &str = "SELECT COUNT(*) AS frames, ROUND(percentile(dur / 1e6, 50), 3) AS p50_ms, ROUND(percentile(dur / 1e6, 95), 3) AS p95_ms, ROUND(MAX(dur) / 1e6, 3) AS max_ms, SUM(dur > 40000000) AS over_40_ms FROM slice WHERE name = 'EA frame'";
const ACTIONS: &str = "WITH act AS (SELECT ts, dur, SUBSTR(name, 14) AS action FROM slice WHERE name GLOB 'egui-tester:*'), frame AS (SELECT ts, dur FROM slice WHERE name = 'EA frame') SELECT action, COUNT(frame.ts) AS frames, ROUND(percentile(frame.dur / 1e6, 50), 3) AS p50_ms, ROUND(percentile(frame.dur / 1e6, 95), 3) AS p95_ms, ROUND(MAX(frame.dur) / 1e6, 3) AS max_ms, SUM(frame.dur > 40000000) AS over_40_ms FROM act LEFT JOIN frame ON frame.ts >= act.ts AND frame.ts < act.ts + act.dur + 500000000 GROUP BY act.ts, action ORDER BY act.ts";
const STAGES: &str = "SELECT name, COUNT(*) AS spans, ROUND(percentile(dur / 1e6, 50), 3) AS p50_ms, ROUND(percentile(dur / 1e6, 95), 3) AS p95_ms, ROUND(MAX(dur) / 1e6, 3) AS max_ms FROM slice WHERE name GLOB 'EA *' OR name GLOB 'HRRR *' GROUP BY name ORDER BY name";
const LONG_FRAMES: &str = "WITH frame AS (SELECT ts, dur FROM slice WHERE name = 'EA frame' AND dur > 40000000), act AS (SELECT ts, dur, SUBSTR(name, 14) AS action FROM slice WHERE name GLOB 'egui-tester:*') SELECT ROUND((frame.ts - (SELECT MIN(ts) FROM slice)) / 1e9, 3) AS relative_s, ROUND(frame.dur / 1e6, 3) AS frame_ms, COALESCE((SELECT action FROM act WHERE frame.ts >= act.ts AND frame.ts < act.ts + act.dur + 500000000 ORDER BY act.ts DESC LIMIT 1), 'unscored') AS action, COALESCE((SELECT name FROM slice AS stage WHERE stage.ts >= frame.ts AND stage.ts + stage.dur <= frame.ts + frame.dur AND stage.name IN ('EA frame.ui', 'EA frame.tessellate', 'EA render.prepare', 'EA render.encode', 'EA render.submit', 'EA render.present') ORDER BY stage.dur DESC LIMIT 1), 'unattributed') AS dominant_stage, COALESCE((SELECT ROUND(stage.dur / 1e6, 3) FROM slice AS stage WHERE stage.ts >= frame.ts AND stage.ts + stage.dur <= frame.ts + frame.dur AND stage.name IN ('EA frame.ui', 'EA frame.tessellate', 'EA render.prepare', 'EA render.encode', 'EA render.submit', 'EA render.present') ORDER BY stage.dur DESC LIMIT 1), 0) AS dominant_ms FROM frame ORDER BY frame.ts";

#[derive(Deserialize)]
struct FrameSummary {
    frames: u64,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    over_40_ms: u64,
}

#[derive(Deserialize)]
struct ActionSummary {
    action: String,
    frames: u64,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    over_40_ms: u64,
}

#[derive(Deserialize)]
struct StageSummary {
    name: String,
    spans: u64,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Deserialize)]
struct LongFrame {
    relative_s: f64,
    frame_ms: f64,
    action: String,
    dominant_stage: String,
    dominant_ms: f64,
}

pub fn write(
    trace_processor: &Path,
    trace: &Path,
    events: &Path,
    output: &Path,
    score: Score,
) -> Result<()> {
    let overall = one::<FrameSummary>(query(trace_processor, trace, OVERALL)?, "frame summary")?;
    let actions = query::<ActionSummary>(trace_processor, trace, ACTIONS)?;
    let stages = query::<StageSummary>(trace_processor, trace, STAGES)?;
    let long_frames = query::<LongFrame>(trace_processor, trace, LONG_FRAMES)?;

    let mut report = String::new();
    line(
        &mut report,
        format_args!("# HRRR Android {score:?} profile"),
    )?;
    line(&mut report, format_args!(""))?;
    line(&mut report, format_args!("- Trace: `{}`", trace.display()))?;
    line(
        &mut report,
        format_args!("- Events: `{}`", events.display()),
    )?;
    line(&mut report, format_args!(""))?;
    line(&mut report, format_args!("## Frame Cadence"))?;
    line(&mut report, format_args!(""))?;
    line(
        &mut report,
        format_args!("| Frames | p50 ms | p95 ms | max ms | >40 ms |"),
    )?;
    line(
        &mut report,
        format_args!("| ---: | ---: | ---: | ---: | ---: |"),
    )?;
    line(
        &mut report,
        format_args!(
            "| {} | {:.3} | {:.3} | {:.3} | {} |",
            overall.frames, overall.p50_ms, overall.p95_ms, overall.max_ms, overall.over_40_ms
        ),
    )?;
    table_actions(&mut report, &actions)?;
    table_stages(&mut report, &stages)?;
    table_long_frames(&mut report, &long_frames)?;

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::Io {
            operation: "create Android profile-report directory",
            path: parent.to_owned(),
            source,
        })?;
    }
    fs::write(output, report).map_err(|source| Error::Io {
        operation: "write Android profile report",
        path: output.to_owned(),
        source,
    })
}

fn query<T: DeserializeOwned>(trace_processor: &Path, trace: &Path, sql: &str) -> Result<Vec<T>> {
    let output = Command::new(trace_processor)
        .args([OsStr::new("query"), trace.as_os_str(), OsStr::new(sql)])
        .output()
        .map_err(|source| Error::Io {
            operation: "query Android Perfetto trace",
            path: trace_processor.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Command {
            command: format!("{} query {}", trace_processor.display(), trace.display()),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    csv::Reader::from_reader(output.stdout.as_slice())
        .deserialize()
        .collect::<std::result::Result<_, _>>()
        .map_err(|source| Error::Verdict {
            detail: format!("decode Perfetto query result: {source}"),
        })
}

fn one<T>(mut rows: Vec<T>, noun: &str) -> Result<T> {
    if rows.len() == 1 {
        Ok(rows.remove(0))
    } else {
        Err(Error::Verdict {
            detail: format!("Perfetto returned {} rows for {noun}", rows.len()),
        })
    }
}

fn table_actions(report: &mut String, rows: &[ActionSummary]) -> Result<()> {
    line(report, format_args!(""))?;
    line(report, format_args!("## Choreographed Acts"))?;
    line(report, format_args!(""))?;
    line(
        report,
        format_args!("| Act | Frames | p50 ms | p95 ms | max ms | >40 ms |"),
    )?;
    line(
        report,
        format_args!("| --- | ---: | ---: | ---: | ---: | ---: |"),
    )?;
    for row in rows {
        line(
            report,
            format_args!(
                "| `{}` | {} | {:.3} | {:.3} | {:.3} | {} |",
                row.action, row.frames, row.p50_ms, row.p95_ms, row.max_ms, row.over_40_ms
            ),
        )?;
    }
    Ok(())
}

fn table_stages(report: &mut String, rows: &[StageSummary]) -> Result<()> {
    line(report, format_args!(""))?;
    line(report, format_args!("## Application Stages"))?;
    line(report, format_args!(""))?;
    line(
        report,
        format_args!("| Stage | Spans | p50 ms | p95 ms | max ms |"),
    )?;
    line(report, format_args!("| --- | ---: | ---: | ---: | ---: |"))?;
    for row in rows {
        line(
            report,
            format_args!(
                "| `{}` | {} | {:.3} | {:.3} | {:.3} |",
                row.name, row.spans, row.p50_ms, row.p95_ms, row.max_ms
            ),
        )?;
    }
    Ok(())
}

fn table_long_frames(report: &mut String, rows: &[LongFrame]) -> Result<()> {
    line(report, format_args!(""))?;
    line(report, format_args!("## Long Frames"))?;
    line(report, format_args!(""))?;
    if rows.is_empty() {
        return line(report, format_args!("No frame exceeded 40 ms."));
    }
    line(
        report,
        format_args!("| Trace s | Frame ms | Act | Dominant leaf stage | Stage ms |"),
    )?;
    line(report, format_args!("| ---: | ---: | --- | --- | ---: |"))?;
    for row in rows {
        line(
            report,
            format_args!(
                "| {:.3} | {:.3} | `{}` | `{}` | {:.3} |",
                row.relative_s, row.frame_ms, row.action, row.dominant_stage, row.dominant_ms
            ),
        )?;
    }
    Ok(())
}

fn line(report: &mut String, arguments: fmt::Arguments<'_>) -> Result<()> {
    fmt::write(report, arguments).map_err(|_| Error::Verdict {
        detail: "format Android profile report".to_owned(),
    })?;
    report.push('\n');
    Ok(())
}
