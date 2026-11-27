use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

use eframe::egui::{self, Response};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, available, define_class, msg_send,
    rc::Retained, sel,
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAccessibilityHighContrastAqua,
    NSAppearanceNameAccessibilityHighContrastDarkAqua, NSAppearanceNameAqua,
    NSAppearanceNameDarkAqua, NSImage, NSMenu, NSMenuItem, NSView,
};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::player::{Control, MAX_SPEED, MIN_SPEED, SPEED_STEP};
use crate::ui::context_menu::{Item, PlayerAction};
use crate::ui::theme::Icon;
use crossbeam_channel::Sender;
use std::rc::Rc;
use std::task::Poll;

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "MyTubeContextMenuTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Cell<Option<usize>>]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(chooseItem:))]
        fn choose(&self, sender: &NSMenuItem) {
            self.ivars().set(usize::try_from(sender.tag()).ok());
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let target = Self::alloc(mtm).set_ivars(Cell::new(None));
        unsafe { msg_send![super(target), init] }
    }
}

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "MyTubeMenuPlaybackRateTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Sender<Control>]
    struct RateTarget;

    impl RateTarget {
        #[unsafe(method(setPlaybackRate:))]
        fn set_rate(&self, rate: f64) {
            let _ = self.ivars().send(Control::Speed(rate));
        }
    }
);

unsafe extern "C" {
    fn mytube_menu_rate_view(
        rate: f64,
        min: f64,
        max: f64,
        step: f64,
        target: *const std::ffi::c_void,
    ) -> *mut NSView;
    fn mytube_catalog_button_create() -> *mut NSView;
    fn mytube_catalog_button_update(
        button: &NSView,
        title: &NSString,
        symbol: &NSString,
        selected: bool,
        hot: bool,
        dark: bool,
        strength: f64,
    );
}

struct CatalogFace {
    view: Retained<NSView>,
    state: Option<(String, &'static str, bool, bool, bool, u8)>,
    enabled: bool,
    dialog: bool,
    pass: u64,
    rect: egui::Rect,
    cuts: Vec<NSRect>,
}
impl Drop for CatalogFace {
    fn drop(&mut self) {
        self.view.removeFromSuperview();
    }
}

#[derive(Default)]
pub(in crate::ui) struct MenuHost {
    view: Option<Retained<NSView>>,
    last_open: Cell<Option<u64>>,
    faces: RefCell<HashMap<egui::Id, CatalogFace>>,
}

impl MenuHost {
    pub fn attach(&mut self, frame: &eframe::Frame) {
        if self.view.is_some() || !available!(macos = 26.0) {
            return;
        }
        let Ok(handle) = frame.window_handle() else {
            return;
        };
        if let RawWindowHandle::AppKit(handle) = handle.as_raw() {
            // eframe owns the root view. Retain it for menus opened from this viewport.
            self.view = unsafe { Retained::retain(handle.ns_view.cast::<NSView>().as_ptr()) };
        }
    }

    pub fn catalog_face(
        &self,
        response: &Response,
        title: &str,
        icon: Option<Icon>,
        selected: bool,
        dialog: bool,
    ) -> bool {
        let ctx = &response.ctx;
        let Some(root) = self.view() else {
            return false;
        };
        if crate::ui::theme::menu_material(ctx) != crate::ui::control_bar::DisplayMaterial::Glass
            || ctx.embed_viewports()
        {
            return false;
        }
        let symbol = match icon {
            None if dialog => "",
            None => "chevron.down",
            Some(Icon::Disk) => "externaldrive",
            Some(Icon::Folder) => "folder",
            Some(Icon::Refresh) => "arrow.clockwise",
            Some(Icon::Filter) => "slider.horizontal.3",
            Some(Icon::Plus) => "plus",
            _ => return false,
        };
        let mut faces = self.faces.borrow_mut();
        let face = faces.entry(response.id).or_insert_with(|| {
            let view = unsafe { Retained::from_raw(mytube_catalog_button_create()) }
                .expect("native catalog button");
            root.addSubview(&view);
            CatalogFace {
                view,
                state: None,
                enabled: response.enabled(),
                dialog,
                pass: 0,
                rect: response.rect,
                cuts: Vec::new(),
            }
        });
        face.pass = ctx.cumulative_pass_nr();
        face.enabled = response.enabled();
        face.rect = response.rect;
        let strength = crate::ui::theme::glass_strength(ctx);
        let hot = response.hovered() || response.has_focus();
        let dark = ctx.theme() == egui::Theme::Dark;
        if !face.state.as_ref().is_some_and(|(text, m, s, h, d, g)| {
            text == title && (*m, *s, *h, *d, *g) == (symbol, selected, hot, dark, strength)
        }) {
            unsafe {
                mytube_catalog_button_update(
                    &face.view,
                    &NSString::from_str(title),
                    &NSString::from_str(symbol),
                    selected,
                    hot,
                    dark,
                    f64::from(strength) / 100.,
                )
            };
            face.state = Some((title.into(), symbol, selected, hot, dark, strength));
        }
        true
    }
    pub fn finish_catalog_buttons(
        &self,
        ctx: &egui::Context,
        modal: bool,
        dialog_view: Option<&NSView>,
    ) {
        let mut faces = self.faces.borrow_mut();
        faces.retain(|_, face| face.pass == ctx.cumulative_pass_nr());
        if faces.is_empty() {
            return;
        }
        let Some(root) = self.view() else { return };
        let covers = ctx.memory(|mem| {
            mem.layer_ids()
                .filter(|layer| layer.order >= egui::Order::Middle && mem.areas().is_visible(layer))
                .filter_map(|layer| mem.area_rect(layer.id))
                .collect::<Vec<_>>()
        });
        for face in faces.values_mut() {
            let parent = if face.dialog {
                let Some(view) = dialog_view else {
                    face.view.setHidden(true);
                    continue;
                };
                view
            } else {
                root
            };
            face.view.setHidden(false);
            if unsafe { face.view.superview() }.as_deref() != Some(parent) {
                parent.addSubview(&face.view);
            }
            let frame = super::settings_glass::view_rect(
                root.bounds(),
                root.isFlipped(),
                ctx.viewport_rect(),
                face.rect,
            );
            let frame = parent.convertRect_fromView(frame, Some(root));
            if face.view.frame() != frame {
                face.view.setFrame(frame);
            }
            let alpha = if !face.enabled {
                0.5
            } else if modal && !face.dialog {
                if ctx.theme() == egui::Theme::Dark {
                    0.64
                } else {
                    0.84
                }
            } else {
                1.0
            };
            if face.view.alphaValue() != alpha {
                face.view.setAlphaValue(alpha);
            }
            if face.dialog {
                continue;
            }
            let cuts = covers
                .iter()
                .map(|cover| cover.intersect(face.rect))
                .filter(|rect| rect.is_positive())
                .map(|rect| {
                    super::settings_glass::view_rect(
                        face.view.bounds(),
                        face.view.isFlipped(),
                        face.rect,
                        rect,
                    )
                })
                .collect::<Vec<_>>();
            if cuts != face.cuts {
                unsafe {
                    super::mytube_catalog_button_clip(&face.view, cuts.as_ptr(), cuts.len());
                }
                face.cuts = cuts;
            }
        }
    }

    pub fn begin_tracking(&self, ctx: &egui::Context) -> bool {
        // egui can repeat a layout pass with the same input after native tracking returns.
        let frame = ctx.cumulative_frame_nr();
        self.last_open.replace(Some(frame)) != Some(frame)
    }

    pub fn window_content(&self, title: &str) -> Option<Retained<NSView>> {
        let root = self.view()?;
        objc2_app_kit::NSApplication::sharedApplication(root.mtm())
            .windows()
            .iter()
            .find(|window| window.title().to_string() == title)?
            .contentView()
    }

    pub fn view(&self) -> Option<&NSView> {
        self.view.as_deref()
    }
}

fn menu<A: Copy>(
    title: &str,
    items: &[Item<A>],
    target: &MenuTarget,
    actions: &mut Vec<A>,
) -> Retained<NSMenu> {
    let mtm = target.mtm();
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    menu.setAutoenablesItems(false);
    for entry in items {
        let item = match entry {
            Item::PlaybackSpeed { current, commands } => {
                let receiver = RateTarget::alloc(mtm).set_ivars(commands.clone());
                let receiver: Retained<RateTarget> = unsafe { msg_send![super(receiver), init] };
                // The custom view retains its receiver throughout AppKit menu tracking.
                let view = unsafe {
                    Retained::from_raw(mytube_menu_rate_view(
                        *current,
                        MIN_SPEED,
                        MAX_SPEED,
                        SPEED_STEP,
                        Retained::as_ptr(&receiver).cast(),
                    ))
                }
                .expect("native menu playback slider");
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str("Playback speed"));
                item.setView(Some(&view));
                item
            }
            Item::Separator => NSMenuItem::separatorItem(mtm),
            Item::Heading(title) | Item::Caption(title) => {
                NSMenuItem::sectionHeaderWithTitle(&NSString::from_str(title), mtm)
            }
            Item::Submenu {
                title,
                enabled,
                items,
            } => {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(title));
                let child = self::menu(title, items, target, actions);
                item.setSubmenu(Some(&child));
                item.setEnabled(*enabled);
                item
            }
            Item::Command {
                title,
                action,
                enabled,
                checked,
                destructive,
            } => {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(title));
                // NSMenuItem's target is weak. The caller owns it until tracking ends.
                unsafe {
                    item.setAction(Some(sel!(chooseItem:)));
                    item.setTarget(Some(target));
                }
                item.setTag(actions.len() as isize);
                actions.push(*action);
                item.setEnabled(*enabled);
                if let Some(checked) = checked {
                    item.setState(isize::from(*checked));
                }
                if *destructive {
                    let trash = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str("trash"),
                        None,
                    );
                    item.setImage(trash.as_deref());
                }
                item
            }
        };
        menu.addItem(&item);
    }
    menu
}

pub(in crate::ui) fn choose<A: Copy>(
    view: &NSView,
    response: &Response,
    entries: &[Item<A>],
) -> Option<A> {
    let point = response
        .interact_pointer_pos()
        .or_else(|| response.ctx.pointer_latest_pos())
        .unwrap_or(response.rect.center());
    choose_at(view, response, entries, point)
}

pub(in crate::ui) fn choose_at<A: Copy>(
    view: &NSView,
    response: &Response,
    entries: &[Item<A>],
    point: egui::Pos2,
) -> Option<A> {
    let target = MenuTarget::new(view.mtm());
    let mut actions = Vec::new();
    let menu = menu("", entries, &target, &mut actions);
    let point = menu_location(&menu, view, response, point);
    // AppKit tracks submenus, keyboard navigation, cancellation and screen edges.
    menu.popUpMenuPositioningItem_atLocation_inView(None, point, Some(view));
    response.ctx.request_repaint();
    target
        .ivars()
        .get()
        .and_then(|index| actions.get(index).copied())
}

fn menu_location(menu: &NSMenu, view: &NSView, response: &Response, point: egui::Pos2) -> NSPoint {
    // Inherit the window's native material whenever the app follows the system.
    // An explicit app theme still needs the corresponding accessibility appearance.
    let theme = response.ctx.theme();
    if response.ctx.system_theme() != Some(theme) {
        use crate::ui::control_bar::DisplayMaterial;
        let contrast = DisplayMaterial::system(&response.ctx) == DisplayMaterial::HighContrast;
        let name = unsafe {
            match (theme, contrast) {
                (egui::Theme::Dark, true) => NSAppearanceNameAccessibilityHighContrastDarkAqua,
                (egui::Theme::Light, true) => NSAppearanceNameAccessibilityHighContrastAqua,
                (egui::Theme::Dark, false) => NSAppearanceNameDarkAqua,
                (egui::Theme::Light, false) => NSAppearanceNameAqua,
            }
        };
        menu.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    }
    let viewport = response.ctx.viewport_rect();
    let pos = point - viewport.min;
    let bounds = view.bounds();
    let x = f64::from(pos.x / viewport.width()) * bounds.size.width;
    let y = f64::from(pos.y / viewport.height()) * bounds.size.height;
    NSPoint::new(
        bounds.origin.x + x,
        bounds.origin.y
            + if view.isFlipped() {
                y
            } else {
                bounds.size.height - y
            },
    )
}

/// Schedule tracking after the current winit event handler returns. AppKit's
/// nested run loop can then deliver redraws to the regular eframe renderer.
pub(in crate::ui) struct PlayerMenu {
    menu: Retained<NSMenu>,
    result: Rc<Cell<Poll<Option<PlayerAction>>>>,
}

impl PlayerMenu {
    pub fn open(view: &NSView, response: &Response, entries: &[Item<PlayerAction>]) -> Self {
        let target = MenuTarget::new(view.mtm());
        let mut actions = Vec::new();
        let menu = menu("", entries, &target, &mut actions);
        let point = response
            .interact_pointer_pos()
            .unwrap_or(response.rect.center());
        let point = menu_location(&menu, view, response, point);
        let result = Rc::new(Cell::new(Poll::Pending));
        let completion = Rc::downgrade(&result);
        let ctx = response.ctx.clone();
        let viewport = ctx.viewport_id();
        let popup = menu.clone();
        let host: Retained<NSView> = Retained::from(view);
        let track = block2::RcBlock::new(move || {
            let Some(completion) = completion.upgrade() else {
                return;
            };
            if host.window().is_some_and(|window| window.isVisible()) {
                popup.popUpMenuPositioningItem_atLocation_inView(None, point, Some(&host));
            }
            let action = target.ivars().get().and_then(|i| actions.get(i).copied());
            completion.set(Poll::Ready(action));
            ctx.request_repaint_of(viewport);
        });
        // The block and its AppKit objects stay on the main thread.
        unsafe { objc2_foundation::NSRunLoop::mainRunLoop().performBlock(&track) };
        Self { menu, result }
    }

    pub fn result(&self) -> Poll<Option<PlayerAction>> {
        self.result.get()
    }
}

impl Drop for PlayerMenu {
    fn drop(&mut self) {
        if self.result.get().is_pending() {
            self.menu.cancelTracking();
        }
    }
}
