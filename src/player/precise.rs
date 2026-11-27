use super::Playback;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreciseId {
    pub(super) load: u64,
    gesture: u64,
}

impl Playback {
    pub fn precise_id(&self, gesture: u64) -> PreciseId {
        PreciseId {
            load: self.load_id,
            gesture,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PreciseFinish {
    Cancel,
    Play(f64),
    Scrub(f64),
}

impl PreciseFinish {
    pub fn position(self) -> Option<f64> {
        match self {
            Self::Cancel => None,
            Self::Play(at) | Self::Scrub(at) => Some(at),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Target {
    Preview(f64),
    Finish { at: f64, paused: bool, commit: bool },
}

impl Target {
    pub fn position(self) -> f64 {
        match self {
            Self::Preview(at) | Self::Finish { at, .. } => at,
        }
    }
}

pub(super) struct PrecisePlayback {
    pub id: PreciseId,
    original: f64,
    was_paused: bool,
    queued: Option<Target>,
    in_flight: Option<Target>,
    saw_seek: bool,
    finishing: bool,
}

impl PrecisePlayback {
    pub fn new(id: PreciseId, state: &Playback, at: f64) -> Self {
        Self {
            id,
            original: state.confirmed_position.unwrap_or(state.position),
            was_paused: state.paused || state.ended,
            queued: Some(Target::Preview(at)),
            in_flight: None,
            saw_seek: false,
            finishing: false,
        }
    }

    pub fn preview(&mut self, at: f64) {
        if !self.finishing {
            self.queued = Some(Target::Preview(at));
        }
    }

    pub fn finish(&mut self, finish: PreciseFinish) {
        if !self.finishing {
            self.finishing = true;
            let at = finish.position();
            self.queued = Some(Target::Finish {
                at: at.unwrap_or(self.original),
                paused: self.was_paused && !matches!(finish, PreciseFinish::Play(_)),
                commit: at.is_some(),
            });
        }
    }

    pub fn next_seek(&mut self) -> Option<Target> {
        if self.in_flight.is_some() {
            return None;
        }
        let next = self.queued.take()?;
        self.in_flight = Some(next);
        self.saw_seek = false;
        Some(next)
    }

    pub fn seeking(&mut self) {
        self.saw_seek = self.in_flight.is_some();
    }

    pub fn restarted(&mut self) -> Option<Target> {
        if !self.saw_seek {
            return None;
        }
        self.saw_seek = false;
        self.in_flight.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubbing_restores_play_state_and_explicit_play_unpauses() {
        for paused in [true, false] {
            let state = Playback {
                paused,
                ..Playback::default()
            };
            for (action, expected_paused) in [
                (PreciseFinish::Scrub(14.0), paused),
                (PreciseFinish::Play(14.0), false),
            ] {
                let mut seek = PrecisePlayback::new(state.precise_id(1), &state, 8.0);
                seek.finish(action);
                assert_eq!(
                    seek.next_seek(),
                    Some(Target::Finish {
                        at: 14.0,
                        paused: expected_paused,
                        commit: true,
                    })
                );
            }
        }
    }

    #[test]
    fn keeps_only_the_latest_preview_and_finishes_at_the_original_position() {
        let state = Playback {
            position: 5.5,
            confirmed_position: Some(5.25),
            paused: true,
            ..Playback::default()
        };
        let mut seek = PrecisePlayback::new(state.precise_id(1), &state, 8.0);
        assert_eq!(seek.next_seek(), Some(Target::Preview(8.0)));
        assert_eq!(seek.restarted(), None);
        for at in [9.0, 10.0, 11.0] {
            seek.preview(at);
        }
        assert_eq!(seek.next_seek(), None);
        seek.seeking();
        assert_eq!(seek.restarted(), Some(Target::Preview(8.0)));
        assert_eq!(seek.next_seek(), Some(Target::Preview(11.0)));
        seek.preview(14.0);
        seek.finish(PreciseFinish::Cancel);
        seek.preview(17.0);
        assert_eq!(seek.next_seek(), None);
        seek.seeking();
        assert_eq!(seek.restarted(), Some(Target::Preview(11.0)));
        let cancel = Target::Finish {
            at: 5.25,
            paused: true,
            commit: false,
        };
        assert_eq!(seek.next_seek(), Some(cancel));
        assert_eq!(seek.restarted(), None);
        seek.seeking();
        assert_eq!(seek.restarted(), Some(cancel));
        assert_eq!(seek.next_seek(), None);
    }
}
