//! A paused, silent decoder on the timeline worker avoids reopening the file
//! and starting FFmpeg for every approximate frame. Exact frames use the normal cache.
use super::{Core, Load, Video, ffi};
use anyhow::{Result, ensure};
use eframe::egui::ColorImage;
use std::{
    ffi::CString,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct DraftDecoder {
    decoder: Option<(Core, (i64, i64))>,
}
impl DraftDecoder {
    pub fn release(&mut self) {
        self.decoder = None;
    }
    pub fn ready_for(&self, video: &Video) -> bool {
        self.decoder
            .as_ref()
            .is_some_and(|(_, key)| *key == video.thumbnail_key())
    }
    fn sample(
        &mut self,
        video: &Video,
        at: f64,
        exact: bool,
        active: &dyn Fn() -> bool,
    ) -> Result<(Vec<u8>, ColorImage)> {
        crate::files::check_file(video)?;
        let key = video.thumbnail_key();
        if self.decoder.as_ref().is_none_or(|(_, k)| *k != key) {
            self.release();
            let core = Core::open()?;
            let scale =
                c"lavfi=[scale=192:144:force_original_aspect_ratio=decrease:force_divisible_by=2]";
            for (name, value) in [
                (c"vo", c"null"),
                (c"audio", c"no"),
                (c"sub", c"no"),
                (c"hwdec", c"no"),
                (c"vd-lavc-threads", c"1"),
                (c"screenshot-format", c"jpg"),
                (c"vf", scale),
            ] {
                core.command_sync(&[c"set", name, value])?;
            }
            core.load(Load {
                video: Box::new(video.clone()),
                start: Some(at),
                paused: true,
                id: 0,
            })?;
            self.decoder = Some((core, key));
        } else if let Some((core, _)) = &self.decoder {
            core.command_sync(&[
                c"seek",
                &CString::new(format!("{at:.4}"))?,
                if exact {
                    c"absolute+exact"
                } else {
                    c"absolute+keyframes"
                },
            ])?;
        }
        let core = &self.decoder.as_ref().unwrap().0;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            ensure!(
                active() && Instant::now() < deadline,
                "Thumbnail request expired"
            );
            // Only this worker reads the dedicated decoder's event queue.
            let event = unsafe { &*(core.api.mpv_wait_event)(core.handle.as_ptr(), 0.01) };
            if event.id == ffi::EVENT_PLAYBACK_RESTART {
                break;
            }
            ensure!(
                event.id != ffi::EVENT_END_FILE && event.id != ffi::EVENT_SHUTDOWN,
                "Thumbnail unavailable"
            );
        }
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("frame.jpg");
        core.command_sync(&[
            c"screenshot-to-file",
            &CString::new(file.to_string_lossy().as_bytes())?,
            c"video",
        ])?;
        crate::files::check_file(video)?;
        let bytes = std::fs::read(file)?;
        let image = crate::media::decode_with_limit(&bytes, 192, 144)?;
        Ok((bytes, image))
    }
    pub fn frame(
        &mut self,
        tool: &Path,
        video: &Video,
        at: f64,
        exact: bool,
        active: &dyn Fn() -> bool,
    ) -> Result<(Vec<u8>, ColorImage)> {
        match self.sample(video, at, exact, active) {
            Ok(image) => Ok(image),
            Err(_) => {
                self.release();
                ensure!(active(), "Thumbnail request superseded");
                crate::media::timeline_preview(tool, video, at, exact, active)
            }
        }
    }
}
