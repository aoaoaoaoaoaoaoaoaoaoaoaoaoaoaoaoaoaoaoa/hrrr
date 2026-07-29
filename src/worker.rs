use crate::{decode, model::*, source::Source, xdg::Lair};
use anyhow::{Context as _, Result, anyhow};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded};
use egui::Context;
use jiff::Timestamp;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const DISCOVERY_POLL: Duration = Duration::from_mins(5);
const DISCOVERY_RETRY: Duration = Duration::from_secs(30);

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

#[derive(Clone, Copy, Debug)]
enum ForgeCommand {
    Survey(RunId),
    Load(LoadDemand),
    Shutdown,
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
    forge: Sender<ForgeCommand>,
    scout: Sender<()>,
    pub events: Receiver<Event>,
    _threads: [thread::JoinHandle<()>; 2],
}

impl Worker {
    pub fn spawn(ctx: Context, lair: &Lair) -> Result<Self> {
        let (forge, forge_rx) = bounded(32);
        let (scout, scout_rx) = bounded(1);
        let (event_tx, events) = bounded(32);
        let forge_source = Source::new(lair);
        let forge_events = event_tx.clone();
        let forge_ctx = ctx.clone();
        let forge_thread = thread::Builder::new()
            .name("hrrr-forge".to_owned())
            .spawn(move || labor(forge_ctx, forge_source, forge_rx, forge_events))
            .context("spawn HRRR field forge")?;
        let scout_source = Source::new(lair);
        let scout_thread = thread::Builder::new()
            .name("hrrr-scout".to_owned())
            .spawn(move || scout_cycles(ctx, scout_source, scout_rx, event_tx))
            .context("spawn HRRR cycle scout")?;
        Ok(Self {
            forge,
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
                .forge
                .send(ForgeCommand::Survey(run))
                .context("send HRRR frontier survey"),
            Command::Load(demand) => self
                .forge
                .send(ForgeCommand::Load(demand))
                .context("send HRRR field demand"),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _sent = self.forge.try_send(ForgeCommand::Shutdown);
    }
}

fn labor(ctx: Context, source: Source, commands: Receiver<ForgeCommand>, events: Sender<Event>) {
    let mut deferred = None;
    while let Some(command) = deferred.take().or_else(|| commands.recv().ok()) {
        let command = coalesce_load(command, &commands, &mut deferred);
        let event =
            match command {
                ForgeCommand::Survey(run) => source
                    .survey(run)
                    .map(Event::Surveyed)
                    .unwrap_or_else(|err| Event::SurveyFault {
                        run,
                        message: format!("{err:#}"),
                    }),
                ForgeCommand::Load(demand) => {
                    let began = Instant::now();
                    source
                        .field_message(demand.key)
                        .and_then(|bytes| decode::field(demand.key, &bytes))
                        .map(|field| Event::Loaded {
                            demand,
                            field: Arc::new(field),
                            elapsed_ms: began.elapsed().as_millis(),
                        })
                        .unwrap_or_else(|err| Event::Fault {
                            demand: Some(demand),
                            message: format!("{err:#}"),
                        })
                }
                ForgeCommand::Shutdown => break,
            };
        if events.send(event).is_err() {
            break;
        }
        ctx.request_repaint();
    }
}

fn scout_cycles(ctx: Context, source: Source, summons: Receiver<()>, events: Sender<Event>) {
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
        ctx.request_repaint();
    }
}

fn coalesce_load(
    command: ForgeCommand,
    commands: &Receiver<ForgeCommand>,
    deferred: &mut Option<ForgeCommand>,
) -> ForgeCommand {
    let ForgeCommand::Load(_) = command else {
        return command;
    };
    let mut newest = command;
    while let Ok(next) = commands.try_recv() {
        match next {
            ForgeCommand::Load(_) => newest = next,
            ForgeCommand::Shutdown => return ForgeCommand::Shutdown,
            ForgeCommand::Survey(_) => {
                *deferred = Some(next);
                break;
            }
        }
    }
    newest
}
