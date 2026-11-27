use super::settings_form::Form;
use crate::player::{
    PlayerTool, Session, TrackKind,
    subtitles::{self, Credentials, Match},
};

use std::sync::mpsc::{Receiver, channel};

enum Reply {
    Matches(Vec<Match>),
    File(tempfile::TempPath),
}

#[derive(Default)]
pub(super) struct OnlineSubtitles {
    key: String,
    token: String,
    query: String,
    language: String,
    load: Option<u64>,
    pending: Option<Receiver<anyhow::Result<Reply>>>,
    results: Vec<Match>,
    files: Vec<tempfile::TempPath>,
    message: String,
}

impl OnlineSubtitles {
    fn request(
        &mut self,
        ui: &Form,
        task: impl FnOnce(Credentials) -> anyhow::Result<Reply> + Send + 'static,
    ) {
        let credentials = Credentials {
            key: self.key.trim().to_owned(),
            token: self.token.trim().to_owned(),
        };
        let (tx, rx) = channel();
        let ctx = ui.ctx().clone();
        self.pending = Some(rx);
        self.message.clear();
        std::thread::spawn(move || {
            let _ = tx.send(task(credentials));
            ctx.request_repaint();
        });
    }

    pub fn update(&mut self, session: Option<&Session>) {
        let load = session.map(Session::load_id);
        if load != self.load {
            self.load = load;
            self.files.clear();
            self.results.clear();
            self.pending = None;
            self.message.clear();
        }
        if let Some(reply) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.pending = None;
            match reply {
                Ok(Reply::Matches(matches)) => {
                    self.results = matches;
                    if self.results.is_empty() {
                        self.message = "No subtitles found".into();
                    }
                }
                Ok(Reply::File(path)) => {
                    if let (Some(session), Some(id)) = (session, load) {
                        session.tool_for(
                            id,
                            PlayerTool::ExternalTrack(TrackKind::Subtitle, path.to_path_buf()),
                        );
                        self.files.push(path);
                    }
                }
                Err(error) => self.message = format!("{error:#}"),
            }
        }
    }

    pub fn show(&mut self, ui: &mut Form) {
        ui.collapsing("Find subtitles on OpenSubtitles", |ui| {
            ui.label("Search sends the title and language to OpenSubtitles. Enter your API key for this session.");
            ui.text(&mut self.key,"OpenSubtitles API key",true);
            ui.text(&mut self.token,"Account token (optional)",true);
            ui.text(&mut self.query,"Movie or episode title",false);
            ui.text(&mut self.language,"Language code, e.g. en",false);
            ui.enabled(self.pending.is_none() && !self.key.trim().is_empty(), |ui| {
                if ui.enabled_button(!self.query.trim().is_empty(),"Search OpenSubtitles").clicked() {
                    let title = self.query.trim().to_owned();
                    let language = self.language.trim().to_owned();
                    self.request(ui, move |auth| subtitles::search(&auth,&title,&language).map(Reply::Matches));
                }
                let selection = self.results.iter().find_map(|entry| ui.button(&entry.title).on_hover_text("Download and load this subtitle").clicked().then_some(entry.id));
                if let Some(id) = selection {
                    self.request(ui, move |auth| subtitles::download(&auth,id).map(Reply::File));
                }
            });
            if self.pending.is_some() { ui.label("Searching…"); }
            if !self.message.is_empty() { ui.label(&self.message); }
        });
    }
}
