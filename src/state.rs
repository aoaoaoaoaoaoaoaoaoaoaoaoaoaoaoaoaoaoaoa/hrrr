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
pub struct Slate {
    pub overlay: Overlay,
    pub cycle: RunSelection,
    pub lead: LeadHour,
    pub base: LeadHour,
    pub active_view: Option<EntryName>,
    pub closed_folders: BTreeSet<String>,
    pub inspector_scroll: f32,
}

impl Default for Slate {
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
enum SlateWire {
    Versioned(VersionedSlate),
    Legacy(LegacySlate),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VersionedSlate {
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
struct LegacySlate {
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

impl Default for LegacySlate {
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

impl Slate {
    pub fn load(path: &Path) -> Result<(Self, bool)> {
        let Some(wire) = load_toml(path, "session state")? else {
            return Ok((Self::default(), false));
        };
        match wire {
            SlateWire::Versioned(wire) => {
                let migrated = wire.schema != SCHEMA;
                Ok((Self::from_versioned(wire)?, migrated))
            }
            SlateWire::Legacy(wire) => Ok((Self::from_legacy(wire)?, true)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let (cycle, fixed_run) = match self.cycle {
            RunSelection::Latest => (CycleMode::Latest, None),
            RunSelection::LatestLong => (CycleMode::LatestLong, None),
            RunSelection::Fixed(run) => (CycleMode::Fixed, Some(run)),
        };
        save_toml(
            &VersionedSlate {
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

    fn from_versioned(wire: VersionedSlate) -> Result<Self> {
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

    fn from_legacy(mut wire: LegacySlate) -> Result<Self> {
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
    fn legacy_qpf_state_rectifies_to_run_accumulation() -> Result<()> {
        let wire = toml::from_str::<SlateWire>("product = \"qpf\"")?;
        let SlateWire::Legacy(wire) = wire else {
            bail!("legacy state parsed as current state");
        };
        let slate = Slate::from_legacy(wire)?;
        assert_eq!(slate.overlay.active(), Some(Product::QpfRun));
        Ok(())
    }

    #[test]
    fn fixed_selection_cannot_exist_without_a_cycle() {
        assert!(refine_cycle(CycleMode::Fixed, None).is_err());
    }

    #[test]
    fn illegal_persisted_leads_are_rejected() {
        let text = "\
schema = 1
overlay = \"smoke\"
cycle = \"latest\"
lead = 49
closed_folders = []
";
        assert!(toml::from_str::<SlateWire>(text).is_err());
    }

    #[test]
    fn inspector_scroll_repels_nonfinite_and_negative_state() -> Result<()> {
        let mut slate = Slate::from_versioned(toml::from_str::<VersionedSlate>(
            "schema = 1\noverlay = \"smoke\"\ncycle = \"latest\"\nlead = 0\nclosed_folders = []\ninspector_scroll = nan\n",
        )?)?;
        assert_eq!(slate.inspector_scroll, 0.0);
        slate.inspector_scroll = lawful_scroll(-8.0);
        assert_eq!(slate.inspector_scroll, 0.0);
        Ok(())
    }

    #[test]
    fn versioned_state_migrates_and_persists_the_base_hour() -> Result<()> {
        let prior = toml::from_str::<VersionedSlate>(
            "schema = 1\noverlay = \"qpf_run\"\ncycle = \"latest\"\nlead = 8\nclosed_folders = []\n",
        )?;
        assert_eq!(prior.base, LeadHour::ZERO);
        let current = Slate::from_versioned(toml::from_str::<VersionedSlate>(
            "schema = 2\noverlay = \"qpf_run\"\ncycle = \"latest\"\nlead = 8\nbase = 3\nclosed_folders = []\n",
        )?)?;
        assert_eq!(current.base, LeadHour::forge(3)?);
        Ok(())
    }
}
