//! The production [`RecorderBackend`]: real devices through `yc-audio`.

use std::time::{Duration, SystemTime};

use crossbeam_channel::Receiver;
use yc_audio::{AudioFrame, RecorderConfig, RecorderEvent, RecorderHandle};
use yc_core::Result;
use yc_core::events::Level;
use yc_core::meeting::{TrackInfo, TrackKind};

use crate::ports::{RecorderBackend, RecordingHandle};

/// Captures microphone and system audio with `yc_audio::Recorder`.
#[derive(Debug, Default, Clone, Copy)]
pub struct YadaRecorder;

impl RecorderBackend for YadaRecorder {
    fn start(&self, config: RecorderConfig) -> Result<Box<dyn RecordingHandle>> {
        Ok(Box::new(YadaHandle(yc_audio::Recorder::start(config)?)))
    }
}

struct YadaHandle(RecorderHandle);

impl RecordingHandle for YadaHandle {
    fn pause(&self) {
        self.0.pause();
    }
    fn resume(&self) {
        self.0.resume();
    }
    fn level(&self, kind: TrackKind) -> Option<Level> {
        self.0.levels().get(kind)
    }
    fn frames(&self, kind: TrackKind) -> Option<Receiver<AudioFrame>> {
        self.0.frames(kind)
    }
    fn events(&self) -> Receiver<RecorderEvent> {
        self.0.events()
    }
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
    fn started_at(&self) -> SystemTime {
        self.0.started_at()
    }
    fn stop(self: Box<Self>) -> Result<Vec<TrackInfo>> {
        self.0.stop()
    }
}
