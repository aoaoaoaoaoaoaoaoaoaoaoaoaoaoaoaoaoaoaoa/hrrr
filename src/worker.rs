use crate::{decode, model::*, source::Source, xdg::Lair};
use anyhow::{Context as _, Result, anyhow};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded};
use eternalist_apps::{
    NativeWake,
    responsiveness::{SupersedingReceiver, SupersedingSender, superseding_channel},
};
use jiff::Timestamp;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const DISCOVERY_POLL: Duration = Duration::from_mins(5);
const DISCOVERY_RETRY: Duration = Duration::from_secs(30);
const BLADE_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DemandId(u64);

impl DemandId {
    pub fn advance(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadIntent {
    Foreground(DemandId),
    Prefetch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadDemand {
    pub intent: LoadIntent,
    pub key: FrameKey,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Discover,
    Survey(RunId),
    Load(LoadDemand),
}

#[derive(Debug)]
pub enum Event {
    Discovered(RunExtent),
    Surveyed(RunExtent),
    SurveyFault {
        run: RunId,
        message: String,
    },
    Loaded {
        demand: LoadDemand,
        field: Arc<FieldGrid>,
        elapsed_ms: u128,
    },
    Fault {
        demand: Option<LoadDemand>,
        message: String,
    },
}

pub struct Worker {
    surveys: SupersedingSender<RunId>,
    loads: SupersedingSender<LoadDemand>,
    shutdown: Sender<()>,
    scout: Sender<()>,
    pub events: Receiver<Event>,
    _threads: [thread::JoinHandle<()>; 2],
}

impl Worker {
    pub fn spawn(ctx: egui::Context, lair: &Lair) -> Result<Self> {
        let (surveys, survey_rx) = superseding_channel();
        let (loads, load_rx) = superseding_channel();
        let (shutdown, shutdown_rx) = bounded(1);
        let (scout, scout_rx) = bounded(1);
        let (event_tx, events) = bounded(32);
        let forge_source = Source::new(lair);
        let forge_events = event_tx.clone();
        let wake = NativeWake::from_context(&ctx);
        let forge_wake = wake.clone();
        let forge_thread = thread::Builder::new()
            .name("hrrr-forge".to_owned())
            .spawn(move || {
                labor(
                    forge_wake,
                    forge_source,
                    shutdown_rx,
                    survey_rx,
                    load_rx,
                    forge_events,
                );
            })
            .context("spawn HRRR field forge")?;
        let scout_source = Source::new(lair);
        let scout_thread = thread::Builder::new()
            .name("hrrr-scout".to_owned())
            .spawn(move || scout_cycles(wake, scout_source, scout_rx, event_tx))
            .context("spawn HRRR cycle scout")?;
        Ok(Self {
            surveys,
            loads,
            shutdown,
            scout,
            events,
            _threads: [forge_thread, scout_thread],
        })
    }

    pub fn send(&self, command: Command) -> Result<()> {
        match command {
            Command::Discover => match self.scout.try_send(()) {
                Ok(()) | Err(TrySendError::Full(())) => Ok(()),
                Err(TrySendError::Disconnected(())) => Err(anyhow!("HRRR cycle scout has fallen")),
            },
            Command::Survey(run) => self
                .surveys
                .offer(run)
                .map(|_superseded| ())
                .map_err(|_| anyhow!("HRRR field forge has fallen")),
            Command::Load(demand) => self
                .loads
                .offer(demand)
                .map(|_superseded| ())
                .map_err(|_| anyhow!("HRRR field forge has fallen")),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _sent = self.shutdown.try_send(());
    }
}

fn labor(
    wake: NativeWake,
    source: Source,
    shutdown: Receiver<()>,
    surveys: SupersedingReceiver<RunId>,
    loads: SupersedingReceiver<LoadDemand>,
    events: Sender<Event>,
) {
    let mut blades = BladeBank::new(BLADE_CAPACITY);
    loop {
        let demand = crossbeam_channel::select_biased! {
            recv(shutdown) -> _ => break,
            recv(surveys.channel()) -> survey => match survey {
                Ok(run) => ForgeDemand::Survey(run),
                Err(_) => break,
            },
            recv(loads.channel()) -> load => match load {
                Ok(load) => ForgeDemand::Load(load),
                Err(_) => break,
            },
        };
        let event = match demand {
            ForgeDemand::Survey(run) => {
                source
                    .survey(run)
                    .map(Event::Surveyed)
                    .unwrap_or_else(|err| Event::SurveyFault {
                        run,
                        message: format!("{err:#}"),
                    })
            }
            ForgeDemand::Load(demand) => {
                let began = Instant::now();
                forge_frame(&source, &mut blades, demand.key)
                    .map(|field| Event::Loaded {
                        demand,
                        field,
                        elapsed_ms: began.elapsed().as_millis(),
                    })
                    .unwrap_or_else(|err| Event::Fault {
                        demand: Some(demand),
                        message: format!("{err:#}"),
                    })
            }
        };
        if events.send(event).is_err() {
            break;
        }
        let _woken = wake.request_foreground_repaint();
    }
}

enum ForgeDemand {
    Survey(RunId),
    Load(LoadDemand),
}

fn forge_frame(source: &Source, blades: &mut BladeBank, key: FrameKey) -> Result<Arc<FieldGrid>> {
    let crown = forge_recipe(source, blades, key, key.valid)?;
    let Some(baseline) = key.baseline() else {
        return Ok(crown);
    };
    if baseline == LeadHour::ZERO {
        return Ok(crown);
    }
    let root = forge_recipe(source, blades, key, baseline)?;
    Ok(Arc::new(crown.increment_since(&root)?))
}

fn forge_recipe(
    source: &Source,
    blades: &mut BladeBank,
    key: FrameKey,
    lead: LeadHour,
) -> Result<Arc<FieldGrid>> {
    match key.product.shape() {
        FieldShape::Scalar => blades.load(
            source,
            key.blade_at(lead, Ingredient::Scalar)
                .context("scalar recipe has no scalar blade")?,
        ),
        FieldShape::Vector => {
            let eastward = blades.load(
                source,
                key.blade_at(lead, Ingredient::Eastward)
                    .context("vector recipe has no eastward blade")?,
            )?;
            let northward = blades.load(
                source,
                key.blade_at(lead, Ingredient::Northward)
                    .context("vector recipe has no northward blade")?,
            )?;
            Ok(Arc::new(FieldGrid::forge_vector(&eastward, &northward)?))
        }
    }
}

struct BladeBank {
    capacity: usize,
    fields: HashMap<BladeKey, Arc<FieldGrid>>,
    order: VecDeque<BladeKey>,
}

impl BladeBank {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            fields: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn load(&mut self, source: &Source, key: BladeKey) -> Result<Arc<FieldGrid>> {
        if let Some(field) = self.fields.get(&key).cloned() {
            self.touch(key);
            return Ok(field);
        }
        let bytes = source.field_message(key)?;
        let field = Arc::new(decode::field(key, &bytes)?);
        let _replaced = self.fields.insert(key, field.clone());
        self.touch(key);
        while self.fields.len() > self.capacity {
            let victim = self
                .order
                .pop_front()
                .context("nonempty blade bank has no eviction order")?;
            let _evicted = self.fields.remove(&victim);
        }
        Ok(field)
    }

    fn touch(&mut self, key: BladeKey) {
        if let Some(slot) = self.order.iter().position(|candidate| *candidate == key) {
            let _prior = self.order.remove(slot);
        }
        self.order.push_back(key);
    }
}

fn scout_cycles(wake: NativeWake, source: Source, summons: Receiver<()>, events: Sender<Event>) {
    let mut next_discovery = Instant::now() + DISCOVERY_POLL;
    loop {
        match summons.recv_timeout(next_discovery.saturating_duration_since(Instant::now())) {
            Ok(()) => while summons.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let (event, cadence) = match source.discover(Timestamp::now()) {
            Ok(extent) => (Event::Discovered(extent), DISCOVERY_POLL),
            Err(err) => (
                Event::Fault {
                    demand: None,
                    message: format!("{err:#}"),
                },
                DISCOVERY_RETRY,
            ),
        };
        next_discovery = Instant::now() + cadence;
        if events.send(event).is_err() {
            break;
        }
        let _woken = wake.request_foreground_repaint();
    }
}
