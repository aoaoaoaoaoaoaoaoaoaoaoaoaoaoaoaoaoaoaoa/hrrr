//! Product-owned semantic score shared by HRRR interaction projections.

use std::time::Duration;

use egui_tester::{Choreography, Result};

/// One platform projection of the HRRR semantic act vocabulary.
pub trait HrrrProjection: Choreography {
    fn enact(&mut self, act: Act) -> Result<()>;
}

/// One product act, before any platform chooses targets or HID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Act {
    SelectPanel(Panel),
    LowerInspector,
    RaiseInspector,
    NextPanel(Panel),
    PreviousPanel(Panel),
    OpenManagedSurface(ManagedSurface),
    CloseManagedSurface(ManagedSurface),
    ToggleWaterEffects,
    PanMap,
    ZoomMap(Zoom),
    PlaceProbe,
    MoveProbe,
    ClearProbe,
    PlacePin,
    DragPin,
    RemovePin,
    UndoMapChange,
    SelectField(Field),
    StepForecastHour(Direction),
    DragForecastHour,
    DragBaseHour,
    FollowRun(RunTether),
    StepRun(Direction),
    ManageViews(ViewAct),
}

impl Act {
    const fn feature(self) -> Feature {
        match self {
            Self::SelectPanel(_) => Feature::PanelSelection,
            Self::LowerInspector | Self::RaiseInspector => Feature::InspectorMotion,
            Self::NextPanel(_) | Self::PreviousPanel(_) => Feature::PanelCarousel,
            Self::OpenManagedSurface(ManagedSurface::Settings)
            | Self::CloseManagedSurface(ManagedSurface::Settings) => Feature::Settings,
            Self::OpenManagedSurface(ManagedSurface::CommandGuide)
            | Self::CloseManagedSurface(ManagedSurface::CommandGuide) => Feature::CommandGuide,
            Self::ToggleWaterEffects => Feature::WaterSetting,
            Self::PanMap => Feature::MapPan,
            Self::ZoomMap(_) => Feature::MapZoom,
            Self::PlaceProbe | Self::MoveProbe | Self::ClearProbe => Feature::MapProbe,
            Self::PlacePin | Self::RemovePin => Feature::MapPin,
            Self::DragPin => Feature::MapPinDrag,
            Self::UndoMapChange => Feature::MapUndo,
            Self::SelectField(_) => Feature::FieldSelection,
            Self::StepForecastHour(_) | Self::DragForecastHour => Feature::ForecastHour,
            Self::DragBaseHour => Feature::ForecastBase,
            Self::FollowRun(_) => Feature::RunTether,
            Self::StepRun(_) => Feature::RunStep,
            Self::ManageViews(ViewAct::Create | ViewAct::RenameActive) => Feature::ActiveView,
            Self::ManageViews(
                ViewAct::Clone
                | ViewAct::RenameEntry
                | ViewAct::Load
                | ViewAct::DragEntryToFolder
                | ViewAct::DragEntryToRoot
                | ViewAct::DeleteEntry,
            ) => Feature::CabinetEntry,
            Self::ManageViews(
                ViewAct::CreateFolder
                | ViewAct::RenameFolder
                | ViewAct::ToggleFolder
                | ViewAct::DragFolder
                | ViewAct::DeleteFolder,
            ) => Feature::CabinetFolder,
        }
    }
}

/// One editorial or semantic beat in a platform-neutral score.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Beat {
    Chapter(&'static str),
    Act(Act),
    Hold(Duration),
}

/// Maintained HRRR score selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Score {
    Comprehensive,
    Fields,
    Forecast,
    Inspector,
    Map,
    Panels,
    Views,
    Water,
}

impl Score {
    #[must_use]
    pub const fn beats(self) -> &'static [Beat] {
        match self {
            Self::Comprehensive => COMPREHENSIVE,
            Self::Fields => FIELDS,
            Self::Forecast => FORECAST,
            Self::Inspector => INSPECTOR,
            Self::Map => MAP,
            Self::Panels => PANELS,
            Self::Views => VIEWS,
            Self::Water => WATER,
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "comprehensive" => Ok(Self::Comprehensive),
            "fields" => Ok(Self::Fields),
            "forecast" => Ok(Self::Forecast),
            "inspector" => Ok(Self::Inspector),
            "map" => Ok(Self::Map),
            "panels" => Ok(Self::Panels),
            "views" => Ok(Self::Views),
            "water" => Ok(Self::Water),
            _ => Err(egui_tester::Error::Verdict {
                detail: format!(
                    "unknown score `{value}`; expected comprehensive, fields, forecast, inspector, map, panels, views, or water"
                ),
            }),
        }
    }
}

/// Execute one score through any lawful HRRR platform projection.
pub fn perform(projection: &mut impl HrrrProjection, score: Score) -> Result<()> {
    for beat in score.beats() {
        match *beat {
            Beat::Chapter(title) => projection.chapter(title)?,
            Beat::Act(act) => projection.enact(act)?,
            Beat::Hold(duration) => projection.hold(duration)?,
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Panel {
    Application,
    Field,
    Forecast,
    ActiveView,
    Views,
    Status,
}

impl Panel {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Application => "Application",
            Self::Field => "Field",
            Self::Forecast => "Forecast",
            Self::ActiveView => "Active View",
            Self::Views => "Views",
            Self::Status => "Status",
        }
    }

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Application => 0,
            Self::Field => 1,
            Self::Forecast => 2,
            Self::ActiveView => 3,
            Self::Views => 4,
            Self::Status => 5,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Field {
    QpfTotal,
    QpfHourly,
    SurfaceSmoke,
    Temperature,
    DewPoint,
    CloudCover,
    SeaLevelPressure,
    Wind,
    AirQuality,
}

impl Field {
    const ALL_MASK: u16 = (1 << 9) - 1;

    const fn mask(self) -> u16 {
        1 << self as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedSurface {
    Settings,
    CommandGuide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Zoom {
    In,
    Out,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunTether {
    Latest,
    LatestLong,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewAct {
    Create,
    RenameActive,
    Clone,
    RenameEntry,
    Load,
    DragEntryToFolder,
    DragEntryToRoot,
    DeleteEntry,
    CreateFolder,
    RenameFolder,
    ToggleFolder,
    DragFolder,
    DeleteFolder,
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum Feature {
    PanelSelection,
    InspectorMotion,
    PanelCarousel,
    Settings,
    CommandGuide,
    WaterSetting,
    MapPan,
    MapZoom,
    MapProbe,
    MapPin,
    MapPinDrag,
    MapUndo,
    FieldSelection,
    ForecastHour,
    ForecastBase,
    RunTether,
    RunStep,
    ActiveView,
    CabinetEntry,
    CabinetFolder,
}

impl Feature {
    const ALL_MASK: u32 = (1 << 20) - 1;

    const fn mask(self) -> u32 {
        1 << self as u8
    }
}

const fn coverage(beats: &[Beat]) -> u32 {
    let mut mask = 0;
    let mut index = 0;
    while index < beats.len() {
        if let Beat::Act(act) = beats[index] {
            mask |= act.feature().mask();
        }
        index += 1;
    }
    mask
}

const fn field_coverage(beats: &[Beat]) -> u16 {
    let mut mask = 0;
    let mut index = 0;
    while index < beats.len() {
        if let Beat::Act(Act::SelectField(field)) = beats[index] {
            mask |= field.mask();
        }
        index += 1;
    }
    mask
}

const INSPECTOR: &[Beat] = &[
    Beat::Chapter("Inspector motion"),
    Beat::Act(Act::SelectPanel(Panel::Application)),
    Beat::Act(Act::LowerInspector),
    Beat::Act(Act::RaiseInspector),
];

const PANELS: &[Beat] = &[
    Beat::Chapter("Panel carousel"),
    Beat::Act(Act::SelectPanel(Panel::Application)),
    Beat::Act(Act::NextPanel(Panel::Field)),
    Beat::Act(Act::NextPanel(Panel::Forecast)),
    Beat::Act(Act::NextPanel(Panel::ActiveView)),
    Beat::Act(Act::NextPanel(Panel::Views)),
    Beat::Act(Act::NextPanel(Panel::Status)),
    Beat::Act(Act::PreviousPanel(Panel::Views)),
    Beat::Act(Act::PreviousPanel(Panel::ActiveView)),
    Beat::Act(Act::PreviousPanel(Panel::Forecast)),
    Beat::Act(Act::PreviousPanel(Panel::Field)),
    Beat::Act(Act::PreviousPanel(Panel::Application)),
];

const WATER: &[Beat] = &[
    Beat::Chapter("Wet panel carousel"),
    Beat::Act(Act::SelectPanel(Panel::Application)),
    Beat::Act(Act::OpenManagedSurface(ManagedSurface::Settings)),
    Beat::Act(Act::ToggleWaterEffects),
    Beat::Act(Act::CloseManagedSurface(ManagedSurface::Settings)),
    Beat::Act(Act::NextPanel(Panel::Field)),
    Beat::Act(Act::NextPanel(Panel::Forecast)),
    Beat::Act(Act::NextPanel(Panel::ActiveView)),
    Beat::Act(Act::NextPanel(Panel::Views)),
    Beat::Act(Act::NextPanel(Panel::Status)),
    Beat::Act(Act::PreviousPanel(Panel::Views)),
    Beat::Act(Act::PreviousPanel(Panel::ActiveView)),
    Beat::Act(Act::PreviousPanel(Panel::Forecast)),
    Beat::Act(Act::PreviousPanel(Panel::Field)),
    Beat::Act(Act::PreviousPanel(Panel::Application)),
    Beat::Act(Act::OpenManagedSurface(ManagedSurface::Settings)),
    Beat::Act(Act::ToggleWaterEffects),
    Beat::Act(Act::CloseManagedSurface(ManagedSurface::Settings)),
];

const MAP: &[Beat] = &[
    Beat::Chapter("Map navigation"),
    Beat::Act(Act::LowerInspector),
    Beat::Act(Act::PanMap),
    Beat::Act(Act::ZoomMap(Zoom::Out)),
    Beat::Act(Act::ZoomMap(Zoom::In)),
    Beat::Act(Act::PlaceProbe),
    Beat::Act(Act::MoveProbe),
    Beat::Act(Act::ClearProbe),
    Beat::Act(Act::UndoMapChange),
    Beat::Act(Act::ClearProbe),
    Beat::Act(Act::PlacePin),
    Beat::Act(Act::DragPin),
    Beat::Act(Act::UndoMapChange),
    Beat::Act(Act::RemovePin),
    Beat::Act(Act::UndoMapChange),
    Beat::Act(Act::RemovePin),
];

const FIELDS: &[Beat] = &[
    Beat::Chapter("Forecast fields"),
    Beat::Act(Act::SelectPanel(Panel::Field)),
    Beat::Act(Act::SelectField(Field::QpfTotal)),
    Beat::Act(Act::SelectField(Field::QpfHourly)),
    Beat::Act(Act::SelectField(Field::SurfaceSmoke)),
    Beat::Act(Act::SelectField(Field::Temperature)),
    Beat::Act(Act::SelectField(Field::DewPoint)),
    Beat::Act(Act::SelectField(Field::CloudCover)),
    Beat::Act(Act::SelectField(Field::SeaLevelPressure)),
    Beat::Act(Act::SelectField(Field::Wind)),
    Beat::Act(Act::SelectField(Field::AirQuality)),
];

const FORECAST: &[Beat] = &[
    Beat::Chapter("Forecast controls"),
    Beat::Act(Act::RaiseInspector),
    Beat::Act(Act::SelectPanel(Panel::Field)),
    Beat::Act(Act::SelectField(Field::QpfTotal)),
    Beat::Act(Act::SelectPanel(Panel::Forecast)),
    Beat::Act(Act::StepForecastHour(Direction::Next)),
    Beat::Act(Act::StepForecastHour(Direction::Previous)),
    Beat::Act(Act::DragForecastHour),
    Beat::Act(Act::DragBaseHour),
    Beat::Act(Act::FollowRun(RunTether::LatestLong)),
    Beat::Act(Act::FollowRun(RunTether::Latest)),
    Beat::Act(Act::StepRun(Direction::Previous)),
    Beat::Act(Act::StepRun(Direction::Next)),
];

const VIEWS: &[Beat] = &[
    Beat::Chapter("Views and folders"),
    Beat::Act(Act::RaiseInspector),
    Beat::Act(Act::SelectPanel(Panel::ActiveView)),
    Beat::Act(Act::ManageViews(ViewAct::RenameActive)),
    Beat::Act(Act::ManageViews(ViewAct::Create)),
    Beat::Act(Act::SelectPanel(Panel::Views)),
    Beat::Act(Act::ManageViews(ViewAct::CreateFolder)),
    Beat::Act(Act::ManageViews(ViewAct::CreateFolder)),
    Beat::Act(Act::ManageViews(ViewAct::RenameFolder)),
    Beat::Act(Act::ManageViews(ViewAct::ToggleFolder)),
    Beat::Act(Act::ManageViews(ViewAct::ToggleFolder)),
    Beat::Act(Act::ManageViews(ViewAct::RenameEntry)),
    Beat::Act(Act::ManageViews(ViewAct::Clone)),
    Beat::Act(Act::ManageViews(ViewAct::Load)),
    Beat::Act(Act::ManageViews(ViewAct::DragEntryToFolder)),
    Beat::Act(Act::ManageViews(ViewAct::DragEntryToRoot)),
    Beat::Act(Act::ManageViews(ViewAct::DragFolder)),
    Beat::Act(Act::ManageViews(ViewAct::DeleteEntry)),
    Beat::Act(Act::ManageViews(ViewAct::DeleteFolder)),
];

const COMPREHENSIVE: &[Beat] = &[
    Beat::Chapter("Application surfaces"),
    Beat::Act(Act::SelectPanel(Panel::Application)),
    Beat::Act(Act::LowerInspector),
    Beat::Act(Act::RaiseInspector),
    Beat::Act(Act::OpenManagedSurface(ManagedSurface::Settings)),
    Beat::Act(Act::ToggleWaterEffects),
    Beat::Act(Act::CloseManagedSurface(ManagedSurface::Settings)),
    Beat::Chapter("Wet panel carousel"),
    Beat::Act(Act::NextPanel(Panel::Field)),
    Beat::Act(Act::NextPanel(Panel::Forecast)),
    Beat::Act(Act::NextPanel(Panel::ActiveView)),
    Beat::Act(Act::NextPanel(Panel::Views)),
    Beat::Act(Act::NextPanel(Panel::Status)),
    Beat::Act(Act::PreviousPanel(Panel::Views)),
    Beat::Act(Act::PreviousPanel(Panel::ActiveView)),
    Beat::Act(Act::PreviousPanel(Panel::Forecast)),
    Beat::Act(Act::PreviousPanel(Panel::Field)),
    Beat::Act(Act::PreviousPanel(Panel::Application)),
    Beat::Act(Act::OpenManagedSurface(ManagedSurface::CommandGuide)),
    Beat::Act(Act::CloseManagedSurface(ManagedSurface::CommandGuide)),
    Beat::Act(Act::OpenManagedSurface(ManagedSurface::Settings)),
    Beat::Act(Act::ToggleWaterEffects),
    Beat::Act(Act::CloseManagedSurface(ManagedSurface::Settings)),
    Beat::Chapter("Every forecast field"),
    Beat::Act(Act::SelectPanel(Panel::Field)),
    Beat::Act(Act::SelectField(Field::QpfTotal)),
    Beat::Act(Act::SelectField(Field::QpfHourly)),
    Beat::Act(Act::SelectField(Field::SurfaceSmoke)),
    Beat::Act(Act::SelectField(Field::Temperature)),
    Beat::Act(Act::SelectField(Field::DewPoint)),
    Beat::Act(Act::SelectField(Field::CloudCover)),
    Beat::Act(Act::SelectField(Field::SeaLevelPressure)),
    Beat::Act(Act::SelectField(Field::Wind)),
    Beat::Act(Act::SelectField(Field::AirQuality)),
    Beat::Act(Act::SelectField(Field::QpfTotal)),
    Beat::Chapter("Forecast time and run"),
    Beat::Act(Act::SelectPanel(Panel::Forecast)),
    Beat::Act(Act::StepForecastHour(Direction::Next)),
    Beat::Act(Act::StepForecastHour(Direction::Previous)),
    Beat::Act(Act::DragForecastHour),
    Beat::Act(Act::DragBaseHour),
    Beat::Act(Act::FollowRun(RunTether::LatestLong)),
    Beat::Act(Act::FollowRun(RunTether::Latest)),
    Beat::Act(Act::StepRun(Direction::Previous)),
    Beat::Act(Act::StepRun(Direction::Next)),
    Beat::Chapter("Map objects and navigation"),
    Beat::Act(Act::LowerInspector),
    Beat::Act(Act::PanMap),
    Beat::Act(Act::ZoomMap(Zoom::Out)),
    Beat::Act(Act::ZoomMap(Zoom::In)),
    Beat::Act(Act::PlaceProbe),
    Beat::Act(Act::MoveProbe),
    Beat::Act(Act::ClearProbe),
    Beat::Act(Act::UndoMapChange),
    Beat::Act(Act::ClearProbe),
    Beat::Act(Act::PlacePin),
    Beat::Act(Act::DragPin),
    Beat::Act(Act::UndoMapChange),
    Beat::Act(Act::RemovePin),
    Beat::Act(Act::UndoMapChange),
    Beat::Act(Act::RemovePin),
    Beat::Chapter("Views and folders"),
    Beat::Act(Act::RaiseInspector),
    Beat::Act(Act::SelectPanel(Panel::ActiveView)),
    Beat::Act(Act::ManageViews(ViewAct::RenameActive)),
    Beat::Act(Act::ManageViews(ViewAct::Create)),
    Beat::Act(Act::SelectPanel(Panel::Views)),
    Beat::Act(Act::ManageViews(ViewAct::CreateFolder)),
    Beat::Act(Act::ManageViews(ViewAct::CreateFolder)),
    Beat::Act(Act::ManageViews(ViewAct::RenameFolder)),
    Beat::Act(Act::ManageViews(ViewAct::ToggleFolder)),
    Beat::Act(Act::ManageViews(ViewAct::ToggleFolder)),
    Beat::Act(Act::ManageViews(ViewAct::RenameEntry)),
    Beat::Act(Act::ManageViews(ViewAct::Clone)),
    Beat::Act(Act::ManageViews(ViewAct::Load)),
    Beat::Act(Act::ManageViews(ViewAct::DragEntryToFolder)),
    Beat::Act(Act::ManageViews(ViewAct::DragEntryToRoot)),
    Beat::Act(Act::ManageViews(ViewAct::DragFolder)),
    Beat::Act(Act::ManageViews(ViewAct::DeleteEntry)),
    Beat::Act(Act::ManageViews(ViewAct::DeleteFolder)),
    Beat::Act(Act::SelectPanel(Panel::Status)),
    Beat::Chapter("Final surface"),
    Beat::Hold(Duration::from_millis(700)),
];

const _: () = assert!(coverage(COMPREHENSIVE) == Feature::ALL_MASK);
const _: () = assert!(field_coverage(COMPREHENSIVE) == Field::ALL_MASK);
