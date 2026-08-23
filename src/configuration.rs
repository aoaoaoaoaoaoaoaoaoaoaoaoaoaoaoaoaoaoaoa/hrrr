use crate::{
    persist::{load_toml, save_toml},
    view::ViewLibrary,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    pub close_minimizes: bool,
}

impl eternalist_apps::configuration::Configuration for Configuration {}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            close_minimizes: true,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigurationWire {
    close_minimizes: bool,
    views: Option<ViewLibrary>,
}

impl Configuration {
    pub fn migrate_legacy_views(path: &Path) -> Result<Option<ViewLibrary>> {
        let wire = match load_toml::<ConfigurationWire>(path, "legacy configuration") {
            Ok(Some(wire)) => wire,
            Ok(None) | Err(_) => return Ok(None),
        };
        let Some(views) = wire.views else {
            return Ok(None);
        };
        Self {
            close_minimizes: wire.close_minimizes,
        }
        .save(path)?;
        Ok(Some(views))
    }

    fn save(&self, path: &Path) -> Result<()> {
        save_toml(self, path, "serialize configuration")
    }
}

impl Default for ConfigurationWire {
    fn default() -> Self {
        Self {
            close_minimizes: true,
            views: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        library::EntryName,
        model::{MercatorPoint, Viewport},
        view::{SavedView, ViewSlot},
    };
    use anyhow::Context as _;
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn forge() -> Result<Self> {
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let path =
                std::env::temp_dir().join(format!("hrrr-config-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _removed = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn legacy_views_migrate_without_remaining_preferences() -> Result<()> {
        let root = TestRoot::forge()?;
        let path = root.0.join("config.toml");
        let mut views = ViewLibrary::default();
        let view = views.saved.first_mut().context("default view")?;
        view.viewport.zoom = 9.25;
        view.slot = ViewSlot::forge(7);
        view.pins
            .push(MercatorPoint::forge([0.25, 0.4]).context("map pin")?);
        let legacy = toml::to_string(&ConfigurationWire {
            close_minimizes: false,
            views: Some(views),
        })?;
        std::fs::write(&path, legacy)?;

        let views = Configuration::migrate_legacy_views(&path)?.context("legacy views")?;
        let view = views.saved.first().context("restored view")?;
        assert_eq!(view.name.as_str(), "default");
        assert_eq!(view.viewport.zoom, 9.25);
        assert_eq!(view.slot.map(ViewSlot::digit), Some(7));
        assert_eq!(view.pins.len(), 1);

        let text = std::fs::read_to_string(path)?;
        assert!(text.contains("close_minimizes = false"));
        assert!(!text.contains("[views]"));
        Ok(())
    }

    #[test]
    fn legacy_duplicate_slots_are_disarmed_without_losing_views() -> Result<()> {
        let root = TestRoot::forge()?;
        let path = root.0.join("views.toml");
        let mut views = ViewLibrary::default();
        views.saved[0].slot = ViewSlot::forge(3);
        let mut second = SavedView::forge(
            EntryName::forge("second").context("second view name")?,
            Viewport::default(),
            Vec::new(),
        );
        second.slot = ViewSlot::forge(3);
        views.saved.push(second);
        views.save(&path)?;
        let (views, _migrated) = ViewLibrary::load(&path, None)?;
        assert_eq!(views.saved[0].slot.map(ViewSlot::digit), Some(3));
        assert_eq!(views.saved[1].slot, None);
        Ok(())
    }
}
