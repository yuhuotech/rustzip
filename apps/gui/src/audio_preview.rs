use rodio::{OutputStreamBuilder, Sink, buffer::SamplesBuffer};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AudioKey {
    pub archive: PathBuf,
    pub path: String,
    pub generation: u64,
}

pub(crate) enum AudioEvent {
    Ended(AudioKey),
    Failed(AudioKey),
}

enum Command {
    Play {
        key: AudioKey,
        data: Arc<[u8]>,
        cancel: Arc<AtomicBool>,
    },
    Stop,
    Shutdown,
}

pub(crate) struct AudioPlayer {
    commands: Sender<Command>,
    events: Receiver<AudioEvent>,
    active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

impl AudioPlayer {
    pub fn new() -> std::io::Result<Self> {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let active_cancel = Arc::new(Mutex::new(None));
        let worker_cancel = active_cancel.clone();
        thread::Builder::new()
            .name("rustzip-audio".into())
            .spawn(move || run_player(command_rx, event_tx, worker_cancel))?;
        Ok(Self {
            commands,
            events,
            active_cancel,
        })
    }

    pub fn play(&self, key: AudioKey, data: Arc<[u8]>) {
        self.cancel_active();
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut active) = self.active_cancel.lock() {
            *active = Some(cancel.clone());
        }
        if self
            .commands
            .send(Command::Play { key, data, cancel })
            .is_err()
        {
            self.cancel_active();
        }
    }

    pub fn stop(&self) {
        self.cancel_active();
        let _ = self.commands.send(Command::Stop);
    }

    fn cancel_active(&self) {
        if let Ok(active) = self.active_cancel.lock()
            && let Some(cancel) = active.as_ref()
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn drain_events(&self) -> Vec<AudioEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.cancel_active();
        let _ = self.commands.send(Command::Shutdown);
    }
}

fn run_player(
    commands: Receiver<Command>,
    events: Sender<AudioEvent>,
    active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
) {
    let mut pending = None;
    loop {
        let command = pending.take().map(Ok).unwrap_or_else(|| commands.recv());
        match command {
            Ok(Command::Play { key, data, cancel }) => {
                pending = play_one(key, data, cancel, &commands, &events, &active_cancel);
            }
            Ok(Command::Stop) => {}
            Ok(Command::Shutdown) | Err(_) => return,
        }
    }
}

fn play_one(
    key: AudioKey,
    data: Arc<[u8]>,
    cancel: Arc<AtomicBool>,
    commands: &Receiver<Command>,
    events: &Sender<AudioEvent>,
    active_cancel: &Mutex<Option<Arc<AtomicBool>>>,
) -> Option<Command> {
    let decoded = match rz_audio_preview::decode_audio(data, &cancel) {
        Ok(decoded) => decoded,
        Err(rz_audio_preview::DecodeError::Cancelled) => {
            clear_active_cancel(active_cancel, &cancel);
            let _ = events.send(AudioEvent::Ended(key));
            return None;
        }
        Err(_) => {
            clear_active_cancel(active_cancel, &cancel);
            let _ = events.send(AudioEvent::Failed(key));
            return None;
        }
    };
    if cancel.load(Ordering::Relaxed) {
        clear_active_cancel(active_cancel, &cancel);
        let _ = events.send(AudioEvent::Ended(key));
        return None;
    }
    let stream = match OutputStreamBuilder::open_default_stream() {
        Ok(stream) => stream,
        Err(_) => {
            clear_active_cancel(active_cancel, &cancel);
            let _ = events.send(AudioEvent::Failed(key));
            return None;
        }
    };
    let sink = Sink::connect_new(stream.mixer());
    sink.append(SamplesBuffer::new(
        decoded.channels,
        decoded.sample_rate,
        decoded.samples,
    ));

    loop {
        if cancel.load(Ordering::Relaxed) {
            sink.stop();
            clear_active_cancel(active_cancel, &cancel);
            let _ = events.send(AudioEvent::Ended(key));
            return None;
        }
        if sink.empty() {
            clear_active_cancel(active_cancel, &cancel);
            let _ = events.send(AudioEvent::Ended(key));
            return None;
        }
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Play {
                key: next_key,
                data,
                cancel: next_cancel,
            }) => {
                sink.stop();
                clear_active_cancel(active_cancel, &cancel);
                let _ = events.send(AudioEvent::Ended(key));
                return Some(Command::Play {
                    key: next_key,
                    data,
                    cancel: next_cancel,
                });
            }
            Ok(Command::Stop) => {
                sink.stop();
                clear_active_cancel(active_cancel, &cancel);
                let _ = events.send(AudioEvent::Ended(key));
                return None;
            }
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                sink.stop();
                clear_active_cancel(active_cancel, &cancel);
                return None;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn clear_active_cancel(active: &Mutex<Option<Arc<AtomicBool>>>, completed: &Arc<AtomicBool>) {
    if let Ok(mut active) = active.lock()
        && active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, completed))
    {
        *active = None;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use rodio::Source as _;

    #[test]
    fn builds_the_pcm_source_without_opening_an_audio_device() {
        let source = SamplesBuffer::new(1, 8_000, vec![0.0f32; 32]);
        assert_eq!(source.total_duration(), Some(Duration::from_millis(4)));
    }
}
