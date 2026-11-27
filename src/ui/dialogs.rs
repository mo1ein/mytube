use std::path::PathBuf;

use chrono::{DateTime, Datelike, Days, Local, Months, NaiveDate, Utc};
use eframe::egui::{
    self, Align, Color32, Context, Id, Key, Layout, Modal, Modifiers, Panel, ScrollArea, Sense,
    TextEdit, Ui, vec2,
};

use super::{
    MyTube, library,
    theme::{self, ButtonKind::*, Icon, button, icon_button, strong, text},
};
use crate::{
    files::{DeleteMode, Operation},
    model::{CollectionTarget, DateRange, Filter, Source, Video},
    scan::Progress,
    service::Action,
};

pub enum Dialog {
    Sources,
    Drive,
    Path(Box<PathEntry>),
    Collection(String, Vec<i64>),
    Filters(Box<FilterDraft>),
    Delete(Vec<Video>),
    ConfirmPermanentDelete(Vec<Video>),
    ResolveFiles(u64, FileIntent, usize),
    FileErrors,
    Media,
}

pub enum PathTarget {
    Source,
    Exclusion,
    Transfer(Vec<Video>, bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileIntent {
    Delete,
    Move,
    Copy,
}

impl FileIntent {
    pub fn dialog(self, videos: Vec<Video>) -> Dialog {
        match self {
            Self::Delete => Dialog::Delete(videos),
            Self::Move => Dialog::path(PathTarget::Transfer(videos, true)),
            Self::Copy => Dialog::path(PathTarget::Transfer(videos, false)),
        }
    }
}

pub struct PathEntry {
    pub target: PathTarget,
    typed: String,
    picked: Option<PathBuf>,
    picking: bool,
}

pub struct FilterDraft {
    filter: Filter,
    dates: [[String; 2]; 3],
    size: String,
}

impl FilterDraft {
    pub fn new(filter: Filter) -> Self {
        let dates = filter.dates.map(|date| {
            [date.from, date.through].map(|value| {
                value
                    .map(|day| day.format("%Y-%m-%d").to_string())
                    .unwrap_or_default()
            })
        });
        let size = filter
            .min_bytes
            .map(|value| format!("{value} B"))
            .unwrap_or_default();
        Self {
            filter,
            dates,
            size,
        }
    }

    fn range(fields: &[String; 2]) -> Option<DateRange> {
        let parse = |text: &str| {
            if text.trim().is_empty() {
                Some(None)
            } else {
                NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
                    .ok()
                    .map(Some)
            }
        };
        let range = DateRange {
            from: parse(&fields[0])?,
            through: parse(&fields[1])?,
        };
        if matches!((range.from, range.through), (Some(from), Some(through)) if from > through) {
            return None;
        }
        Some(range)
    }
}

impl Dialog {
    pub fn path(target: PathTarget) -> Self {
        Self::Path(Box::new(PathEntry {
            target,
            typed: String::new(),
            picked: None,
            picking: false,
        }))
    }
}

impl PathEntry {
    pub fn complete_pick(&mut self, path: Option<PathBuf>) {
        self.picking = false;
        if let Some(path) = path {
            self.typed = path.display().to_string();
            self.picked = Some(path);
        }
    }

    fn candidate(&self) -> Option<PathBuf> {
        self.picked.clone().or_else(|| {
            let directory = self.typed.trim();
            (!directory.is_empty()).then(|| PathBuf::from(directory))
        })
    }
}

impl MyTube {
    pub fn open_drives(&mut self) {
        self.drives = None;
        self.service.send(Action::Drives);
        self.dialog = Some(Dialog::Drive);
    }

    pub fn show_dialog(&mut self, ctx: &Context, progress: &Progress) {
        let colors = theme::colors(ctx, &ctx.global_style().visuals);
        let Some(mut dialog) = self.dialog.take() else {
            #[cfg(target_os = "macos")]
            self.dialog_glass.hide();
            return;
        };
        let id = match dialog {
            Dialog::Sources => "sources",
            Dialog::Drive => "drives",
            Dialog::Path(_) => "choose_path",
            Dialog::Collection(..) => "new_collection",
            Dialog::Filters(_) => "filters",
            Dialog::Delete(_) => "delete",
            Dialog::ConfirmPermanentDelete(_) => "confirm_permanent_delete",
            Dialog::ResolveFiles(..) => "load_file_selection",
            Dialog::FileErrors => "file_errors",
            Dialog::Media => "media_setup",
        };
        #[cfg(target_os = "macos")]
        let native = self.watch.menu_host.view().is_some()
            && !ctx.embed_viewports()
            && theme::menu_material(ctx) == super::control_bar::DisplayMaterial::Glass
            && !egui::Popup::is_any_open(ctx);
        #[cfg(not(target_os = "macos"))]
        let native = false;
        #[cfg(target_os = "macos")]
        let mut frame_start = 0;
        let source_page = matches!(&dialog, Dialog::Sources | Dialog::Drive)
            || matches!(&dialog, Dialog::Path(entry) if matches!(entry.target, PathTarget::Source | PathTarget::Exclusion));
        let surface = if source_page {
            colors.back
        } else {
            colors.card
        };
        let fill = if native {
            theme::glass_tint(ctx, surface)
        } else if !cfg!(target_os = "macos")
            && source_page
            && theme::interface_theme(ctx).translucent()
            && !theme::solid_glass(ctx)
        {
            surface.gamma_multiply(1. - 0.24 * theme::glass_amount(ctx))
        } else {
            surface
        };
        let radius = if source_page && theme::interface_theme(ctx).glass_ui() {
            20
        } else {
            13
        };
        let shown = Modal::new(Id::new(id))
            .backdrop_color(Color32::from_black_alpha(
                if ctx.theme() == egui::Theme::Light {
                    42
                } else {
                    92
                },
            ))
            .frame(theme::layer(fill, 22, 18).corner_radius(radius))
            .show(ctx, |ui| {
                #[cfg(target_os = "macos")]
                {
                    // Frame::begin reserves the background shape just before its contents.
                    frame_start =
                        ctx.graphics_mut(|g| g.entry(ui.layer_id()).next_idx().0.saturating_sub(1));
                }
                ui.set_width(470.0_f32.min(ctx.content_rect().width() - 56.0));
                if source_page {
                    theme::style_form(ui);
                }
                match &mut dialog {
                    Dialog::Sources => self.sources_dialog(ui, progress),
                    Dialog::Drive => self.drive_dialog(ui),
                    Dialog::Path(state) => self.path_dialog(ui, state),
                    Dialog::Collection(name, videos) => self.collection_dialog(ui, name, videos),
                    Dialog::Filters(filter) => self.filter_dialog(ui, filter),
                    Dialog::Delete(videos) => self.delete_dialog(ui, videos, false),
                    Dialog::ConfirmPermanentDelete(videos) => self.delete_dialog(ui, videos, true),
                    Dialog::ResolveFiles(_, _, count) => {
                        title(
                            ui,
                            "Loading selection",
                            &format!("Reading {count} selected videos."),
                        );
                        ui.spinner();
                        right(ui, |ui| button(ui, "Cancel", None, Secondary).clicked())
                    }
                    Dialog::FileErrors => self.file_errors_dialog(ui),
                    Dialog::Media => self.media_dialog(ui),
                }
            });
        #[cfg(target_os = "macos")]
        if native && !shown.inner && !egui::Popup::is_any_open(ctx) {
            self.dialog_glass.dialog(
                &shown.response,
                frame_start,
                radius.into(),
                self.watch.menu_host.view(),
            );
        } else {
            self.dialog_glass.hide();
        }
        let picking = matches!(&dialog, Dialog::Path(entry) if entry.picking);
        if !shown.inner && (!shown.should_close() || picking) && self.dialog.is_none() {
            self.dialog = Some(dialog);
        }
    }

    pub fn detail_panel(&mut self, ui: &mut Ui) -> bool {
        let colors = theme::palette(ui);
        let Some(video) = self.detail.clone() else {
            return false;
        };
        let width = (ui.available_width() * 0.40).clamp(165.0, 308.0);
        let busy =
            self.file_job.is_some() || self.detail_removal.is_some_and(|(id, _)| id == video.id);
        let mut close = false;
        Panel::right("selected_video")
            .exact_size(width)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::sidebar_frame(12, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(strong("Video details", 15.0, colors.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close = icon_button(ui, "Close details", Icon::Close, Quiet).clicked();
                    });
                });
                ScrollArea::vertical()
                    .id_salt(("details", video.id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(3.0);
                        let (rect, response) = ui.allocate_exact_size(
                            vec2(ui.available_width(), ui.available_width() * 9.0 / 16.0),
                            Sense::hover(),
                        );
                        self.thumbs.preview_artwork(ui, &response, rect, &video);
                        ui.add_space(6.0);
                        ui.add(egui::Label::new(strong(&video.title, 20.0, colors.text)).wrap());
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if button(ui, "Play video", Some(Icon::Play), Primary).clicked() {
                                    self.play(video.clone(), super::PlaybackView::FullPlayer);
                                }
                                super::watch_later_control(
                                    ui,
                                    video.id,
                                    self.settings.watch_later.contains(&video.id),
                                    &self.service,
                                );
                                for (label, icon, moving) in [
                                    ("Move video", Icon::Move, true),
                                    ("Copy video", Icon::Copy, false),
                                ] {
                                    if icon_button(ui, label, icon, Secondary).clicked() {
                                        self.dialog = Some(Dialog::path(PathTarget::Transfer(
                                            vec![video.clone()],
                                            moving,
                                        )));
                                    }
                                }
                                if icon_button(ui, "Delete video", Icon::Trash, Danger).clicked() {
                                    self.dialog = Some(Dialog::Delete(vec![video.clone()]));
                                }
                            });
                        });
                        ui.separator();

                        video_metadata(ui, &video);
                        if button(ui, "Browse this folder", Some(Icon::Folder), Secondary).clicked()
                            && let Some(parent) = video.path.parent()
                        {
                            self.browse_folder(parent);
                        }
                        if button(
                            ui,
                            "Open containing folder",
                            Some(Icon::External),
                            Secondary,
                        )
                        .clicked()
                        {
                            self.service.send(Action::Reveal(video.path.clone()));
                        }
                        ui.separator();
                        ui.label(strong("Collections", 13.0, colors.text));
                        if self.settings.collections.is_empty() {
                            if button(ui, "Create collection", Some(Icon::Plus), Secondary)
                                .clicked()
                            {
                                self.dialog = Some(Dialog::Collection(String::new(), Vec::new()));
                            }
                        } else {
                            for collection in &self.settings.collections {
                                let mut included = self.memberships.contains(&collection.id);
                                if ui.checkbox(&mut included, &collection.name).changed() {
                                    self.service.send(Action::Member(
                                        video.id,
                                        collection.id,
                                        included,
                                    ));
                                }
                            }
                        }
                        ui.add_space(8.0);
                    });
            });
        close
    }

    fn source_action(
        &self,
        ui: &mut Ui,
        label: &str,
        icon: Option<Icon>,
        kind: theme::ButtonKind,
    ) -> egui::Response {
        #[cfg(target_os = "macos")]
        let start = ui.painter().add(egui::Shape::Noop);
        let response = button(ui, label, icon, kind);
        #[cfg(target_os = "macos")]
        if ui.clip_rect().contains_rect(response.rect)
            && !egui::Popup::is_any_open(ui.ctx())
            && self
                .watch
                .menu_host
                .catalog_face(&response, label, icon, kind == Primary, true)
        {
            ui.ctx().graphics_mut(|g| {
                let list = g.entry(ui.layer_id());
                for i in start.0..list.next_idx().0 {
                    list.reset_shape(egui::layers::ShapeIdx(i));
                }
            });
        }
        response
    }

    fn sources_dialog(&mut self, ui: &mut Ui, progress: &Progress) -> bool {
        let colors = theme::palette(ui);
        title(
            ui,
            "Library sources",
            "Manage automatic drives, folder sources, and exclusions.",
        );
        ui.horizontal_wrapped(|ui| {
            if self
                .source_action(ui, "Add folder", Some(Icon::Plus), Primary)
                .clicked()
            {
                self.dialog = Some(Dialog::path(PathTarget::Source));
            }
            if self
                .source_action(ui, "Add drive", Some(Icon::Disk), Secondary)
                .clicked()
            {
                self.open_drives();
            }
            if ui
                .add_enabled_ui(
                    self.settings.auto_scan || !self.settings.sources.is_empty(),
                    |ui| self.source_action(ui, "Refresh", Some(Icon::Refresh), Secondary),
                )
                .inner
                .clicked()
            {
                self.service.rescan();
                self.browser.changed();
            }
        });
        theme::form_group(ui, "Scanning", |ui| {
            let mut automatic = self.settings.auto_scan;
            if theme::setting_toggle(
                ui,
                &mut automatic,
                "Scan mounted local drives automatically",
            )
            .changed()
            {
                self.service.send(Action::AutoScan(automatic));
            }
            if progress.running {
                ui.horizontal(|ui| {
                    ui.label(text(
                        if progress.paused {
                            "Scan paused"
                        } else {
                            "Scanning sources"
                        },
                        12.0,
                        colors.accent,
                    ));
                    self.pause_button(ui, progress);
                });
                ui.add(
                    egui::Label::new(text(
                        format!(
                            "Source {} of {}  ·  {} files checked  ·  {} videos found",
                            progress.source, progress.sources, progress.checked, progress.found
                        ),
                        12.0,
                        colors.muted,
                    ))
                    .wrap(),
                );
            }
        });
        ScrollArea::vertical()
            .id_salt("source_locations")
            .max_height((ui.ctx().content_rect().height() - 330.).clamp(90., 340.))
            .show(ui, |ui| {
                theme::form_group(ui, "Scanned locations", |ui| {
                    self.source_rows(ui, &self.settings.sources, false);
                });
                theme::form_group(ui, "Excluded folders", |ui| {
                    self.source_rows(ui, &self.settings.exclusions, true);
                    if self
                        .source_action(ui, "Add exclusion", Some(Icon::Plus), Secondary)
                        .clicked()
                    {
                        self.dialog = Some(Dialog::path(PathTarget::Exclusion));
                    }
                });
            });
        ui.add_space(7.0);
        right(ui, |ui| {
            self.source_action(ui, "Done", None, Secondary).clicked()
        })
    }

    fn source_rows(&self, ui: &mut Ui, paths: &[Source], exclusion: bool) {
        let colors = theme::palette(ui);
        if paths.is_empty() {
            ui.label(text(
                if exclusion {
                    "No excluded folders"
                } else {
                    "No locations added"
                },
                12.0,
                colors.muted,
            ));
        }
        for source in paths {
            ui.push_id(source.id, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(20.0, 25.0), Sense::hover());
                    let icon = if source.automatic {
                        Icon::Disk
                    } else {
                        Icon::Folder
                    };
                    icon.paint(ui.painter(), rect.center(), 16.0, colors.accent);
                    ui.add_sized(
                        [ui.available_width() - 50.0, 25.0],
                        egui::Label::new(text(source.path.to_string_lossy(), 13.0, colors.text))
                            .truncate(),
                    )
                    .on_hover_text(source.path.to_string_lossy());
                    let label = if exclusion {
                        "Remove exclusion"
                    } else if source.automatic {
                        "Exclude this drive from scans"
                    } else {
                        "Stop scanning this location"
                    };
                    if icon_button(ui, label, Icon::Close, Quiet).clicked() {
                        if source.automatic {
                            self.service
                                .send(Action::AddSource(source.path.clone(), true));
                        } else {
                            self.service
                                .send(Action::RemoveSource(source.id, exclusion));
                        }
                    }
                });
            });
        }
    }

    fn drive_dialog(&mut self, ui: &mut Ui) -> bool {
        let colors = theme::palette(ui);
        title(
            ui,
            "Add a drive",
            "Select a mounted drive to include in your library.",
        );
        let mut chosen = None;
        if let Some(drives) = self.drives.as_ref() {
            if drives.is_empty() {
                ui.label(text(
                    "No mounted drives found. You can enter a folder path instead.",
                    13.0,
                    colors.muted,
                ));
            }
            ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for path in drives {
                    if theme::nav(ui, &path.to_string_lossy(), Icon::Disk, false, false).clicked() {
                        chosen = Some(path.clone());
                    }
                }
            });
        } else {
            ui.spinner();
        }
        ui.add_space(7.0);
        if let Some(path) = chosen {
            self.service.send(Action::AddSource(path, false));
            self.dialog = Some(Dialog::Sources);
        }
        right(ui, |ui| {
            if self.source_action(ui, "Back", None, Secondary).clicked() {
                self.dialog = Some(Dialog::Sources);
            }
        });
        false
    }

    fn path_dialog(&mut self, ui: &mut Ui, state: &mut PathEntry) -> bool {
        let (heading, explanation, submit) = match &state.target {
            PathTarget::Source => (
                "Add a folder",
                "The folder and its subfolders will be scanned for videos.".to_owned(),
                "Add folder",
            ),
            PathTarget::Exclusion => (
                "Exclude a folder",
                "Videos in this folder and its subfolders will be skipped.".to_owned(),
                "Exclude folder",
            ),
            PathTarget::Transfer(videos, moving) => {
                let verb = if *moving { "Move" } else { "Copy" };
                let subject = if let [video] = videos.as_slice() {
                    format!("“{}”", video.title)
                } else {
                    format!("{} videos", videos.len())
                };
                (
                    if *moving {
                        "Move to folder"
                    } else {
                        "Copy to folder"
                    },
                    format!("{verb} {subject} to the destination folder."),
                    verb,
                )
            }
        };
        title(ui, heading, &explanation);
        if let PathTarget::Transfer(videos, _) = &state.target
            && videos.len() > 1
        {
            file_list(ui, videos);
        }
        let source = matches!(state.target, PathTarget::Source | PathTarget::Exclusion);
        let action = |ui: &mut Ui, label: &str, icon, kind| {
            if source {
                self.source_action(ui, label, icon, kind)
            } else {
                button(ui, label, icon, kind)
            }
        };
        let mut folder_field = |ui: &mut Ui| {
            if ui
                .add(
                    TextEdit::singleline(&mut state.typed)
                        .desired_width(ui.available_width())
                        .hint_text("Enter an absolute folder path"),
                )
                .changed()
            {
                state.picked = None;
            }
            if ui
                .add_enabled_ui(!state.picking, |ui| {
                    action(
                        ui,
                        if state.picking {
                            "Choosing folder…"
                        } else {
                            "Browse folders"
                        },
                        Some(Icon::Folder),
                        Secondary,
                    )
                })
                .inner
                .clicked()
            {
                state.picking = true;
                self.service.pick_folder();
            }
        };
        if source {
            theme::form_group(ui, "Folder location", folder_field);
        } else {
            folder_field(ui);
        }
        ui.add_space(8.0);
        let candidate = state.candidate();
        let valid = candidate.as_ref().is_some_and(|path| path.is_absolute()) && !state.picking;
        let (submit, cancel) = right(ui, |ui| {
            let submit = ui
                .add_enabled_ui(valid, |ui| action(ui, submit, None, Primary))
                .inner
                .clicked();
            let cancel = ui
                .add_enabled_ui(!state.picking, |ui| action(ui, "Cancel", None, Secondary))
                .inner
                .clicked();
            (submit, cancel)
        });
        if submit && let Some(path) = candidate {
            match &state.target {
                PathTarget::Source => return self.service.send(Action::AddSource(path, false)),
                PathTarget::Exclusion => {
                    if self.service.send(Action::AddSource(path, true)) {
                        self.dialog = Some(Dialog::Sources);
                    }
                }
                PathTarget::Transfer(videos, moving) => {
                    return self.start_file_job(
                        videos,
                        Operation::Transfer {
                            destination: path,
                            moving: *moving,
                        },
                    );
                }
            }
        }
        cancel
    }

    fn collection_dialog(&mut self, ui: &mut Ui, name: &mut String, videos: &[i64]) -> bool {
        title(
            ui,
            "New collection",
            "Group related videos without changing their locations.",
        );
        let field = ui.add(
            TextEdit::singleline(name)
                .desired_width(ui.available_width())
                .char_limit(80)
                .hint_text("Collection name"),
        );
        if !ui.memory(|mem| {
            mem.data
                .get_temp::<bool>(ui.id().with("collection_focus"))
                .unwrap_or(false)
        }) {
            field.request_focus();
            ui.memory_mut(|mem| mem.data.insert_temp(ui.id().with("collection_focus"), true));
        }
        let enter = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        right(ui, |ui| {
            let create = ui
                .add_enabled_ui(!name.trim().is_empty(), |ui| {
                    button(ui, "Create collection", None, Primary)
                })
                .inner
                .clicked();
            if (create || enter)
                && !name.trim().is_empty()
                && self.service.send(Action::AddToCollection(
                    CollectionTarget::New(name.clone()),
                    videos.to_vec(),
                    self.detail.as_ref().map(|video| video.id),
                ))
            {
                return true;
            }
            button(ui, "Cancel", None, Secondary).clicked()
        })
    }

    fn filter_dialog(&mut self, ui: &mut Ui, draft: &mut FilterDraft) -> bool {
        let colors = theme::palette(ui);
        let filter = &mut draft.filter;
        title(
            ui,
            "Filter videos",
            "Filter by file details or inclusive date ranges.",
        );
        ScrollArea::vertical()
            .max_height((ui.ctx().content_rect().height() - 160.0).max(160.0))
            .show(ui, |ui| {
                egui::Grid::new("advanced_filters")
                    .num_columns(2)
                    .spacing(vec2(12.0, 8.0))
                    .show(ui, |ui| {
                        for (label, hint, field) in [
                            ("File name", "File name contains…", &mut filter.filename),
                            ("Folder", "Folder path contains…", &mut filter.folder),
                            ("File type", "e.g. mp4", &mut filter.ext),
                            ("Video codec", "e.g. h264", &mut filter.codec),
                        ] {
                            ui.label(text(label, 13.0, colors.muted));
                            ui.add(
                                TextEdit::singleline(field)
                                    .char_limit(240)
                                    .desired_width(220.0)
                                    .hint_text(hint),
                            );
                            ui.end_row();
                        }
                        for (label, value, suffix, max) in [
                            (
                                "Minimum duration",
                                &mut filter.min_seconds,
                                " sec",
                                86_400_000,
                            ),
                            (
                                "Duration under",
                                &mut filter.max_seconds,
                                " sec",
                                86_400_000,
                            ),
                            ("Minimum resolution", &mut filter.min_height, " px", 32_768),
                            ("Resolution below", &mut filter.max_height, " px", 32_768),
                        ] {
                            ui.label(text(label, 13.0, colors.muted));
                            optional_number(ui, value, suffix, max);
                            ui.end_row();
                        }
                        ui.label(text("Minimum file size", 13.0, colors.muted));
                        ui.add(
                            TextEdit::singleline(&mut draft.size)
                                .desired_width(220.0)
                                .char_limit(32)
                                .hint_text("e.g. 50 MB or 1.5 GB"),
                        );
                        ui.end_row();
                        ui.label("");
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [134.0, 22.0],
                                egui::Label::new(text("On or after", 12.0, colors.muted)),
                            );
                            ui.add_sized(
                                [134.0, 22.0],
                                egui::Label::new(text("On or before", 12.0, colors.muted)),
                            );
                        });
                        ui.end_row();
                        for (label, fields) in ["Date added", "Date created", "Date modified"]
                            .into_iter()
                            .zip(&mut draft.dates)
                        {
                            ui.label(text(label, 13.0, colors.muted));
                            ui.push_id(label, |ui| {
                                ui.horizontal(|ui| {
                                    for (index, field) in fields.iter_mut().enumerate() {
                                        let end = if index == 0 {
                                            "on or after"
                                        } else {
                                            "on or before"
                                        };
                                        ui.push_id(index, |ui| {
                                            date_field(ui, field, &format!("{label} {end}"))
                                        });
                                    }
                                })
                            });
                            ui.end_row();
                        }
                    });
            });
        let dates = draft.dates.each_ref().map(FilterDraft::range);
        let dates_valid = dates.iter().all(Option::is_some);
        let size = crate::model::minimum_size(&draft.size);
        if size.is_none() {
            ui.label(text(
                "Enter a size like 50 MB, 1.5 GB, or 512 KiB.",
                12.0,
                colors.text,
            ));
        }
        if !dates_valid {
            ui.label(text(
                "Use YYYY-MM-DD with the earlier date on the left.",
                12.0,
                colors.text,
            ));
        }
        let valid = !matches!((filter.min_seconds, filter.max_seconds), (Some(min), Some(max)) if max <= min)
            && !matches!((filter.min_height, filter.max_height), (Some(min), Some(max)) if max <= min);
        if !valid {
            ui.label(text(
                "The maximum must be greater than the minimum.",
                12.0,
                colors.text,
            ));
        }
        ui.add_space(10.0);
        right(ui, |ui| {
            if ui
                .add_enabled_ui(valid && dates_valid && size.is_some(), |ui| {
                    button(ui, "Apply filters", None, Primary)
                })
                .inner
                .clicked()
            {
                filter.dates = dates.map(Option::unwrap_or_default);
                filter.min_bytes = size.flatten();
                filter.text.clone_from(&self.filter.text);
                filter.directory.clone_from(&self.filter.directory);
                self.filter = filter.clone();
                self.filter_open = advanced_active(&self.filter);
                self.apply_filter();
                return true;
            }
            if button(ui, "Cancel", None, Secondary).clicked() {
                return true;
            }
            if button(ui, "Reset", None, Quiet).clicked() {
                *filter = Filter {
                    sort: filter.sort,
                    ..Filter::default()
                };
                draft.dates = Default::default();
                draft.size.clear();
            }
            false
        })
    }

    fn delete_dialog(&mut self, ui: &mut Ui, videos: &mut Vec<Video>, permanent: bool) -> bool {
        let (forward, backward) = ui.input_mut(|input| {
            let backward = input.count_and_consume_key(Modifiers::SHIFT, Key::Tab)
                + input.count_and_consume_key(Modifiers::NONE, Key::ArrowLeft);
            let forward = input.count_and_consume_key(Modifiers::NONE, Key::Tab)
                + input.count_and_consume_key(Modifiers::NONE, Key::ArrowRight);
            (forward, backward)
        });
        if forward != 0 || backward != 0 {
            // Keep focus inside the action row and avoid activating the old button during a move.
            ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
            ui.input_mut(|input| {
                input.events.retain(|event| {
                    !matches!(
                        event,
                        egui::Event::Key {
                            key: Key::Enter | Key::Space,
                            pressed: true,
                            ..
                        }
                    )
                });
            });
        }
        let direction = forward as isize - backward as isize;
        let subject = if videos.len() == 1 {
            "video".to_owned()
        } else {
            format!("{} videos", videos.len())
        };
        let heading = if permanent {
            format!("Permanently delete {subject}?")
        } else {
            format!("Delete {subject}?")
        };
        title(
            ui,
            &heading,
            if permanent {
                "The selected files will be erased from disk. This cannot be undone."
            } else {
                "Files moved to Trash can be restored."
            },
        );
        file_list(ui, videos);
        ui.add_space(5.0);
        right(ui, |ui| {
            let trash =
                (!permanent).then(|| button(ui, "Move to Trash", Some(Icon::Trash), Primary));
            let delete = button(
                ui,
                "Delete Permanently",
                permanent.then_some(Icon::Trash),
                Danger,
            );
            let cancel = button(ui, "Cancel", None, Secondary);
            let buttons: &[&egui::Response] = if let Some(trash) = &trash {
                &[&cancel, &delete, trash]
            } else {
                &[&cancel, &delete]
            };
            let focused = buttons.iter().position(|button| button.has_focus());
            if direction != 0 || focused.is_none() {
                let next = (focused.unwrap_or(0) as isize + direction)
                    .rem_euclid(buttons.len() as isize) as usize;
                buttons[next].request_focus();
                ui.ctx().request_repaint();
            }

            if trash.as_ref().is_some_and(egui::Response::clicked) {
                return self.start_file_job(videos, Operation::Delete(DeleteMode::Trash));
            }
            if delete.clicked() {
                if permanent {
                    return self.start_file_job(videos, Operation::Delete(DeleteMode::Permanent));
                }
                self.dialog = Some(Dialog::ConfirmPermanentDelete(std::mem::take(videos)));
                return true;
            }
            cancel.clicked()
        })
    }

    fn file_errors_dialog(&self, ui: &mut Ui) -> bool {
        let colors = theme::palette(ui);
        title(
            ui,
            "File operation results",
            &format!("{} files could not be processed.", self.file_errors.len()),
        );
        ScrollArea::vertical().max_height(270.0).show_rows(
            ui,
            46.0,
            self.file_errors.len(),
            |ui, range| {
                for (path, error) in &self.file_errors[range] {
                    ui.allocate_ui(vec2(ui.available_width(), 46.0), |ui| {
                        ui.add(
                            egui::Label::new(strong(path.to_string_lossy(), 12.0, colors.text))
                                .truncate(),
                        )
                        .on_hover_text(path.to_string_lossy());
                        ui.add(egui::Label::new(text(error, 12.0, colors.muted)).truncate())
                            .on_hover_text(error);
                    });
                }
            },
        );
        right(ui, |ui| button(ui, "Done", None, Primary).clicked())
    }

    fn media_dialog(&self, ui: &mut Ui) -> bool {
        let colors = theme::palette(ui);
        title(
            ui,
            "Media setup",
            &format!("MyTube couldn’t find: {}.", self.missing.join(", ")),
        );
        ui.add(
            egui::Label::new(text(
                "Run the MyTube installer again to restore playback, metadata, and previews.",
                13.0,
                colors.muted,
            ))
            .wrap(),
        );
        ui.add_space(9.0);
        right(ui, |ui| button(ui, "Done", None, Primary).clicked())
    }
}

fn file_list(ui: &mut Ui, videos: &[Video]) {
    let colors = theme::palette(ui);
    ScrollArea::vertical()
        .id_salt("file_confirmation")
        .max_height(115.0)
        .show_rows(ui, 22.0, videos.len(), |ui, range| {
            for video in &videos[range] {
                let filename = library::folder_label(&video.path);
                let name = video.path.parent().map_or_else(
                    || filename.clone(),
                    |folder| format!("{filename}  ·  {}", folder.display()),
                );
                ui.add_sized(
                    [ui.available_width(), 22.0],
                    egui::Label::new(text(name, 12.0, colors.text))
                        .halign(Align::Min)
                        .truncate(),
                )
                .on_hover_text(video.path.to_string_lossy());
            }
        });
    ui.add_space(6.0);
}

pub(super) fn video_metadata(ui: &mut Ui, video: &Video) {
    let colors = theme::palette(ui);
    let wide = ui.available_width() >= 540.0;
    egui::Grid::new("video_metadata")
        .num_columns(if wide { 4 } else { 2 })
        .min_row_height(20.0)
        .spacing(vec2(18.0, 4.0))
        .show(ui, |ui| {
            for (index, (name, value)) in metadata(video).into_iter().enumerate() {
                ui.label(text(name, 12.0, colors.muted));
                ui.label(text(value, 13.0, colors.text));
                if !wide || index % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    ui.add_space(8.0);
    ui.label(strong("File location", 12.0, colors.text));
    ui.add(
        egui::Label::new(text(video.path.to_string_lossy(), 12.0, colors.muted))
            .wrap()
            .selectable(true),
    );
}

pub(super) fn metadata(video: &Video) -> [(&'static str, String); 10] {
    let resolution = match (video.media.width, video.media.height) {
        (Some(w), Some(h)) => format!("{w} × {h}"),
        _ => "—".into(),
    };
    let formatted = DateTime::<Utc>::from_timestamp_millis(video.added).map_or_else(
        || "—".into(),
        |date| date.with_timezone(&Local).format("%b %e, %Y").to_string(),
    );
    let modified = DateTime::<Utc>::from_timestamp_nanos(video.stamp.modified)
        .with_timezone(&Local)
        .format("%b %e, %Y")
        .to_string();
    let fps = video
        .media
        .fps
        .map_or_else(|| "—".into(), |fps| format!("{fps:.2} fps"));
    [
        ("Format", video.ext.to_uppercase()),
        ("Duration", library::duration(video.media.duration)),
        ("Resolution", resolution),
        ("Size", library::bytes(video.stamp.bytes)),
        (
            "Video codec",
            video.media.codec.clone().unwrap_or_else(|| "—".into()),
        ),
        (
            "Audio codec",
            video.media.audio.clone().unwrap_or_else(|| "—".into()),
        ),
        ("Frame rate", fps),
        ("Date added", formatted),
        (
            "Created",
            library::created_date(video).unwrap_or_else(|| "Unavailable".into()),
        ),
        ("Modified", modified),
    ]
}

fn date_field(ui: &mut Ui, field: &mut String, label: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add(
            TextEdit::singleline(field)
                .desired_width(94.0)
                .char_limit(10)
                .hint_text("Any date"),
        )
        .on_hover_text(format!("{label} (YYYY-MM-DD)"));
        let calendar = icon_button(ui, &format!("Choose {label}"), Icon::Calendar, Quiet);
        let month_id = calendar.id.with("calendar_month");
        let selected = NaiveDate::parse_from_str(field.trim(), "%Y-%m-%d").ok();
        if calendar.clicked() {
            let date = selected.unwrap_or_else(|| Local::now().date_naive());
            ui.ctx()
                .data_mut(|data| data.insert_temp(month_id, date.with_day(1).unwrap_or(date)));
        }
        egui::Popup::menu(&calendar)
            .style(theme::menu_style(&calendar.ctx))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                let Some(mut month) = ui.ctx().data(|data| data.get_temp::<NaiveDate>(month_id))
                else {
                    return;
                };
                ui.set_min_width(230.0);
                ui.horizontal(|ui| {
                    if icon_button(ui, "Previous month", Icon::Previous, Quiet).clicked() {
                        month = month.checked_sub_months(Months::new(1)).unwrap_or(month);
                    }
                    ui.add_sized(
                        [142.0, 34.0],
                        egui::Label::new(month.format("%B %Y").to_string()),
                    );
                    if icon_button(ui, "Next month", Icon::Next, Quiet).clicked() {
                        month = month.checked_add_months(Months::new(1)).unwrap_or(month);
                    }
                });
                ui.ctx().data_mut(|data| data.insert_temp(month_id, month));
                egui::Grid::new("calendar_days")
                    .num_columns(7)
                    .spacing(vec2(3.0, 3.0))
                    .show(ui, |ui| {
                        for day in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] {
                            ui.label(day);
                        }
                        ui.end_row();
                        let first = i64::from(month.weekday().num_days_from_monday());
                        for cell in 0..42 {
                            let date = u64::try_from(cell - first)
                                .ok()
                                .and_then(|day| month.checked_add_days(Days::new(day)))
                                .filter(|date| date.month() == month.month());
                            if let Some(day) = date {
                                let response = ui
                                    .add_sized(
                                        [29.0, 27.0],
                                        egui::Button::selectable(
                                            selected == Some(day),
                                            day.day().to_string(),
                                        ),
                                    )
                                    .on_hover_text(day.format("%A, %B %-d, %Y").to_string());
                                if response.clicked() {
                                    *field = day.format("%Y-%m-%d").to_string();
                                    ui.close();
                                }
                            } else {
                                ui.allocate_space(vec2(29.0, 27.0));
                            }
                            if cell % 7 == 6 {
                                ui.end_row();
                            }
                        }
                    });
                if ui.button("Clear date").clicked() {
                    field.clear();
                    ui.close();
                }
            });
    });
}

fn title(ui: &mut Ui, heading: &str, summary: &str) {
    let colors = theme::palette(ui);
    ui.label(strong(heading, 22.0, colors.text));
    ui.add(egui::Label::new(text(summary, 13.0, colors.muted)).wrap());
    ui.add_space(8.0);
}

fn right<T>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> T) -> T {
    ui.with_layout(Layout::right_to_left(Align::Center), content)
        .inner
}

fn optional_number(ui: &mut Ui, value: &mut Option<i64>, suffix: &str, max: i64) -> bool {
    let mut enabled = value.is_some();
    let mut number = value.unwrap_or(1);
    let mut change = false;
    ui.horizontal(|ui| {
        change |= ui.checkbox(&mut enabled, "").changed();
        change |= ui
            .add_enabled(
                enabled,
                egui::DragValue::new(&mut number)
                    .range(1..=max)
                    .suffix(suffix),
            )
            .changed();
    });
    if change {
        *value = enabled.then_some(number);
    }
    change
}

fn advanced_active(filter: &Filter) -> bool {
    !filter.filename.is_empty()
        || !filter.folder.is_empty()
        || !filter.codec.is_empty()
        || filter.added_since.is_some()
        || filter
            .dates
            .iter()
            .any(|range| *range != DateRange::default())
        || filter.min_bytes.is_some()
        || filter.min_seconds.is_some()
        || filter.max_seconds.is_some()
        || filter.min_height.is_some()
        || filter.max_height.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{player::options::InterfaceTheme, service::Service};

    #[test]
    fn source_manager_and_folder_picker_follow_the_library_appearance() {
        let ctx = Context::default();
        let service = Service::start_with(ctx.clone(), || anyhow::bail!("isolated sources test"));
        let mut app = MyTube::new(&ctx, None, false, service);
        ctx.all_styles_mut(|s| s.animation_time = 0.);
        for appearance in [egui::Theme::Light, egui::Theme::Dark] {
            ctx.set_theme(appearance);
            for style in InterfaceTheme::ALL {
                theme::apply_style(&ctx, style, 62);
                let base = theme::colors(&ctx, &ctx.style_of(appearance).visuals).back;
                for destination in [
                    Dialog::Sources,
                    Dialog::path(PathTarget::Source),
                    Dialog::Drive,
                ] {
                    app.dialog = Some(destination);
                    let mut draw = || {
                        ctx.run_ui(Default::default(), |_| {
                            app.show_dialog(&ctx, &Progress::default())
                        })
                    };
                    draw().drop_without_applying_deltas();
                    let output = draw();
                    let fill = if !cfg!(target_os = "macos")
                        && style.translucent()
                        && !theme::solid_glass(&ctx)
                    {
                        base.gamma_multiply(1. - 0.24 * theme::glass_amount(&ctx))
                    } else {
                        base
                    };
                    let surfaces = output
                        .shapes
                        .iter()
                        .filter_map(|s| match &s.shape {
                            egui::Shape::Rect(r) if r.rect.width() > 400. => {
                                Some((r.rect.size(), r.fill))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    assert!(
                        surfaces.iter().any(|(_, color)| *color == fill),
                        "wrong {style:?} source page surface in {appearance:?}: {surfaces:?} expected {fill:?}"
                    );
                    output.drop_without_applying_deltas();
                }
            }
        }
    }
}
