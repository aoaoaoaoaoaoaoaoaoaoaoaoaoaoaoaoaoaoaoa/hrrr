use crate::{
    library::EntryName,
    model::{LeadHour, MercatorPoint, Overlay, Product, RunId, RunSelection, Viewport},
    persist::{load_toml, save_toml},
};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

const SCHEMA: u16 = 2;

#[derive(Clone, Debug)]
pub struct SessionState {
    pub overlay: Overlay,
    pub cycle: RunSelection,
    pub lead: LeadHour,
    pub base: LeadHour,
    pub active_view: Option<EntryName>,
    pub closed_folders: BTreeSet<String>,
    pub inspector_scroll: f32,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            overlay: Product::default().into(),
            cycle: RunSelection::default(),
            lead: LeadHour::ZERO,
            base: LeadHour::ZERO,
            active_view: None,
            closed_folders: BTreeSet::new(),
            inspector_scroll: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CycleMode {
    #[default]
    Latest,
    LatestLong,
    Fixed,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SessionStateWire {
    Versioned(VersionedSessionState),
    Legacy(LegacySessionState),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VersionedSessionState {
    schema: u16,
    #[serde(alias = "product")]
    overlay: Overlay,
    cycle: CycleMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fixed_run: Option<RunId>,
    lead: LeadHour,
    #[serde(default)]
    base: LeadHour,
    active_view: Option<EntryName>,
    closed_folders: BTreeSet<String>,
    #[serde(default)]
    inspector_scroll: f32,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacySessionState {
    #[serde(alias = "product")]
    overlay: Overlay,
    cycle_tether: CycleMode,
    run: Option<RunId>,
    lead: LeadHour,
    viewport: Viewport,
    #[serde(alias = "probes")]
    pins: Vec<MercatorPoint>,
    active_view: Option<EntryName>,
    closed_folders: BTreeSet<String>,
}

impl Default for LegacySessionState {
    fn default() -> Self {
        Self {
            overlay: Product::default().into(),
            cycle_tether: CycleMode::Latest,
            run: None,
            lead: LeadHour::ZERO,
            viewport: Viewport::default(),
            pins: Vec::new(),
            active_view: None,
            closed_folders: BTreeSet::new(),
        }
    }
}

impl SessionState {
    pub fn load(path: &Path) -> Result<(Self, bool)> {
        let Some(wire) = load_toml(path, "session state")? else {
            return Ok((Self::default(), false));
        };
        match wire {
            SessionStateWire::Versioned(wire) => {
                let migrated = wire.schema != SCHEMA;
                Ok((Self::from_versioned(wire)?, migrated))
            }
            SessionStateWire::Legacy(wire) => Ok((Self::from_legacy(wire)?, true)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let (cycle, fixed_run) = match self.cycle {
            RunSelection::Latest => (CycleMode::Latest, None),
            RunSelection::LatestLong => (CycleMode::LatestLong, None),
            RunSelection::Fixed(run) => (CycleMode::Fixed, Some(run)),
        };
        save_toml(
            &VersionedSessionState {
                schema: SCHEMA,
                overlay: self.overlay,
                cycle,
                fixed_run,
                lead: self.lead,
                base: self.base,
                active_view: self.active_view.clone(),
                closed_folders: self.closed_folders.clone(),
                inspector_scroll: self.inspector_scroll,
            },
            path,
            "serialize session state",
        )
    }

    fn from_versioned(wire: VersionedSessionState) -> Result<Self> {
        if !matches!(wire.schema, 1 | SCHEMA) {
            bail!("unsupported session-state schema {}", wire.schema);
        }
        let cycle = refine_cycle(wire.cycle, wire.fixed_run)?;
        Ok(Self {
            overlay: wire.overlay,
            cycle,
            lead: wire.lead,
            base: wire.base,
            active_view: wire.active_view,
            closed_folders: wire.closed_folders,
            inspector_scroll: lawful_scroll(wire.inspector_scroll),
        })
    }

    fn from_legacy(mut wire: LegacySessionState) -> Result<Self> {
        wire.viewport.normalize();
        let _legacy_pins = wire
            .pins
            .drain(..)
            .filter_map(MercatorPoint::normalize)
            .collect::<Vec<_>>();
        let cycle = refine_cycle(wire.cycle_tether, wire.run)?;
        Ok(Self {
            overlay: wire.overlay,
            cycle,
            lead: wire.lead,
            base: LeadHour::ZERO,
            active_view: wire.active_view,
            closed_folders: wire.closed_folders,
            inspector_scroll: 0.0,
        })
    }
}

fn lawful_scroll(offset: f32) -> f32 {
    if offset.is_finite() {
        offset.max(0.0)
    } else {
        0.0
    }
}

fn refine_cycle(mode: CycleMode, fixed: Option<RunId>) -> Result<RunSelection> {
    match (mode, fixed) {
        (CycleMode::Latest, _) => Ok(RunSelection::Latest),
        (CycleMode::LatestLong, _) => Ok(RunSelection::LatestLong),
        (CycleMode::Fixed, Some(run)) => Ok(RunSelection::Fixed(run)),
        (CycleMode::Fixed, None) => bail!("fixed cycle selection has no cycle"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_versions_rectify_legacy_qpf_and_introduce_the_base_hour() -> Result<()> {
        let wire = toml::from_str::<SessionStateWire>("product = \"qpf\"")?;
        let SessionStateWire::Legacy(wire) = wire else {
            bail!("legacy state parsed as current state");
        };
        let session_state = SessionState::from_legacy(wire)?;
        assert_eq!(session_state.overlay.active(), Some(Product::QpfRun));

        let prior = toml::from_str::<VersionedSessionState>(
            "schema = 1\noverlay = \"qpf_run\"\ncycle = \"latest\"\nlead = 8\nclosed_folders = []\n",
        )?;
        assert_eq!(prior.base, LeadHour::ZERO);
        let current = SessionState::from_versioned(toml::from_str::<VersionedSessionState>(
            "schema = 2\noverlay = \"qpf_run\"\ncycle = \"latest\"\nlead = 8\nbase = 3\nclosed_folders = []\n",
        )?)?;
        assert_eq!(current.base, LeadHour::forge(3)?);
        Ok(())
    }

    #[test]
    fn persisted_selection_rejects_missing_cycles_and_illegal_leads() {
        assert!(refine_cycle(CycleMode::Fixed, None).is_err());
        let text = "\
schema = 1
overlay = \"smoke\"
cycle = \"latest\"
lead = 73
closed_folders = []
";
        assert!(toml::from_str::<SessionStateWire>(text).is_err());
    }

    #[test]
    fn inspector_scroll_repels_nonfinite_and_negative_state() -> Result<()> {
        let mut session_state = SessionState::from_versioned(toml::from_str::<
            VersionedSessionState,
        >(
            "schema = 1\noverlay = \"smoke\"\ncycle = \"latest\"\nlead = 0\nclosed_folders = []\ninspector_scroll = nan\n",
        )?)?;
        assert_eq!(session_state.inspector_scroll, 0.0);
        session_state.inspector_scroll = lawful_scroll(-8.0);
        assert_eq!(session_state.inspector_scroll, 0.0);
        Ok(())
    }
}
