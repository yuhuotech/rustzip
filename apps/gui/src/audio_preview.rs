use rodio::{Decoder, OutputStreamBuilder, Sink, Source};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

const MAX_PLAYBACK_DURATION: Duration = Duration::from_secs(15 * 60);

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
    Play { key: AudioKey, data: Arc<[u8]> },
    Stop,
    Shutdown,
}

pub(crate) struct AudioPlayer {
    commands: Sender<Command>,
    events: Receiver<AudioEvent>,
}

impl AudioPlayer {
    pub fn new() -> std::io::Result<Self> {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        thread::Builder::new()
            .name("rustzip-audio".into())
            .spawn(move || run_player(command_rx, event_tx))?;
        Ok(Self { commands, events })
    }

    pub fn play(&self, key: AudioKey, data: Arc<[u8]>) {
        let _ = self.commands.send(Command::Play { key, data });
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn drain_events(&self) -> Vec<AudioEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

fn run_player(commands: Receiver<Command>, events: Sender<AudioEvent>) {
    let mut pending = None;
    loop {
        let command = pending.take().map(Ok).unwrap_or_else(|| commands.recv());
        match command {
            Ok(Command::Play { key, data }) => {
                pending = play_one(key, data, &commands, &events);
            }
            Ok(Command::Stop) => {}
            Ok(Command::Shutdown) | Err(_) => return,
        }
    }
}

fn play_one(
    key: AudioKey,
    data: Arc<[u8]>,
    commands: &Receiver<Command>,
    events: &Sender<AudioEvent>,
) -> Option<Command> {
    let stream = match OutputStreamBuilder::open_default_stream() {
        Ok(stream) => stream,
        Err(_) => {
            let _ = events.send(AudioEvent::Failed(key));
            return None;
        }
    };
    let decoder = match Decoder::new(Cursor::new(data)) {
        Ok(decoder) => decoder.take_duration(MAX_PLAYBACK_DURATION),
        Err(_) => {
            let _ = events.send(AudioEvent::Failed(key));
            return None;
        }
    };
    let sink = Sink::connect_new(stream.mixer());
    sink.append(decoder);

    loop {
        if sink.empty() {
            let _ = events.send(AudioEvent::Ended(key));
            return None;
        }
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Play {
                key: next_key,
                data,
            }) => {
                sink.stop();
                let _ = events.send(AudioEvent::Ended(key));
                return Some(Command::Play {
                    key: next_key,
                    data,
                });
            }
            Ok(Command::Stop) => {
                sink.stop();
                let _ = events.send(AudioEvent::Ended(key));
                return None;
            }
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                sink.stop();
                return None;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn decodes_a_bounded_pcm_wave_without_an_audio_device() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&68u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&32u32.to_le_bytes());
        wav.extend_from_slice(&[128; 32]);

        let mut decoder = Decoder::new(Cursor::new(Arc::<[u8]>::from(wav)))
            .unwrap()
            .take_duration(MAX_PLAYBACK_DURATION);
        assert!(decoder.next().is_some());
    }
}
