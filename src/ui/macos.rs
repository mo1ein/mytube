pub(super) mod context_menu;
pub(super) mod controller_glass;
pub(super) mod library_glass;
pub(super) mod media_buttons;
pub(super) mod settings_glass;

use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path, ptr::NonNull};

use crossbeam_channel::{Receiver, unbounded};
use eframe::egui::{Context, Rect};
use objc2::{
    AnyThread, MainThreadMarker, class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, ProtocolObject},
};
use objc2_app_kit::{
    NSApplication, NSSharingServicePicker, NSWorkspace,
    NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSNotification, NSNotificationCenter, NSNumber, NSObjectProtocol,
    NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL,
};

use super::control_bar::DisplayMaterial;

unsafe extern "C" {
    fn mytube_catalog_button_clip(view: &objc2_app_kit::NSView, cuts: *const NSRect, count: usize);
}

pub(super) struct GlassAccessibility {
    center: Retained<NSNotificationCenter>,
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl GlassAccessibility {
    pub fn new(ctx: &Context) -> Self {
        refresh_material(ctx);
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let ctx = ctx.clone();
        let callback = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
            refresh_material(&ctx);
        });
        // The system notification can arrive on any thread. The callback captures only egui's
        // thread-safe context and retains no AppKit objects.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
                None,
                &callback,
            )
        };
        Self { center, observer }
    }
}

impl Drop for GlassAccessibility {
    fn drop(&mut self) {
        // This token was registered with the same center and the callback remains thread-safe.
        unsafe { self.center.removeObserver((*self.observer).as_ref()) };
    }
}

fn refresh_material(ctx: &Context) {
    let workspace = NSWorkspace::sharedWorkspace();
    let material = if workspace.accessibilityDisplayShouldIncreaseContrast() {
        DisplayMaterial::HighContrast
    } else if workspace.accessibilityDisplayShouldReduceTransparency()
        || !objc2::available!(macos = 26.0)
    {
        DisplayMaterial::Opaque
    } else {
        DisplayMaterial::Glass
    };
    material.apply(ctx);
}

pub(super) fn share_file(path: &Path, button: Rect, viewport: Rect) -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let Some(view) = NSApplication::sharedApplication(main_thread)
        .keyWindow()
        .and_then(|window| window.contentView())
    else {
        return false;
    };
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // NSURL reads the NUL-terminated filesystem path during this call.
    let url = unsafe {
        NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
            NonNull::from(path.as_bytes_with_nul()).cast(),
            false,
            None,
        )
    };
    let items = NSArray::from_slice(&[url.as_ref()]);
    // NSURL implements NSPasteboardWriting and carries a local file URL.
    let picker =
        unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items) };
    let bounds = view.bounds();
    let scale_x = bounds.size.width / f64::from(viewport.width());
    let scale_y = bounds.size.height / f64::from(viewport.height());
    let size = NSSize::new(
        f64::from(button.width()) * scale_x,
        f64::from(button.height()) * scale_y,
    );
    let position = button.min - viewport.min;
    let x = bounds.origin.x + f64::from(position.x) * scale_x;
    let y = f64::from(position.y) * scale_y;
    let (y, edge) = if view.isFlipped() {
        (bounds.origin.y + y, NSRectEdge::MaxY)
    } else {
        (
            bounds.origin.y + bounds.size.height - y - size.height,
            NSRectEdge::MinY,
        )
    };
    picker.showRelativeToRect_ofView_preferredEdge(
        NSRect::new(NSPoint::new(x, y), size),
        &view,
        edge,
    );
    true
}

#[link(name = "MediaPlayer", kind = "framework")]
unsafe extern "C" {
    static MPMediaItemPropertyTitle: &'static NSString;
    static MPMediaItemPropertyPlaybackDuration: &'static NSString;
    static MPNowPlayingInfoPropertyElapsedPlaybackTime: &'static NSString;
    static MPNowPlayingInfoPropertyPlaybackRate: &'static NSString;
    static MPNowPlayingInfoPropertyMediaType: &'static NSString;
}

#[derive(Clone, Copy)]
pub(super) enum RemoteAction {
    Play,
    Pause,
    Toggle,
    Seek(f64),
    Previous,
    Next,
}

struct RemoteCommand {
    command: Retained<AnyObject>,
    token: Retained<AnyObject>,
}

impl RemoteCommand {
    fn enable(&self, enabled: bool) {
        unsafe { msg_send![&*self.command, setEnabled: Bool::new(enabled)] }
    }
}

impl Drop for RemoteCommand {
    fn drop(&mut self) {
        self.enable(false);
        unsafe { msg_send![&*self.command, removeTarget: &*self.token] }
    }
}

pub(super) struct NowPlaying {
    info: Retained<AnyObject>,
    seek: RemoteCommand,
    previous: RemoteCommand,
    next: RemoteCommand,
    _transport: [RemoteCommand; 3],
    events: Receiver<RemoteAction>,
    availability: Option<(bool, bool, bool)>,
    published: Option<(i64, f64, f64, usize)>,
}

impl NowPlaying {
    pub fn new(ctx: &Context) -> Self {
        let (sender, events) = unbounded();
        // These selectors and the block signature are defined by MediaPlayer's public headers.
        let info = unsafe { msg_send![class!(MPNowPlayingInfoCenter), defaultCenter] };
        let center: Retained<AnyObject> =
            unsafe { msg_send![class!(MPRemoteCommandCenter), sharedCommandCenter] };
        let install = |command: Retained<AnyObject>, action| {
            let send = sender.clone();
            let ctx = ctx.clone();
            let callback = block2::RcBlock::new(move |event: NonNull<AnyObject>| -> isize {
                let action = if matches!(action, RemoteAction::Seek(_)) {
                    let position: f64 = unsafe { msg_send![event.as_ptr(), positionTime] };
                    if !position.is_finite() {
                        return 200;
                    }
                    RemoteAction::Seek(position)
                } else {
                    action
                };
                if send.send(action).is_err() {
                    return 110;
                }
                ctx.request_repaint();
                0
            });
            let token = unsafe { msg_send![&*command, addTargetWithHandler: &*callback] };
            RemoteCommand { command, token }
        };
        let play = install(
            unsafe { msg_send![&*center, playCommand] },
            RemoteAction::Play,
        );
        let pause = install(
            unsafe { msg_send![&*center, pauseCommand] },
            RemoteAction::Pause,
        );
        let toggle = install(
            unsafe { msg_send![&*center, togglePlayPauseCommand] },
            RemoteAction::Toggle,
        );
        for command in [&play, &pause, &toggle] {
            command.enable(true);
        }
        let seek = install(
            unsafe { msg_send![&*center, changePlaybackPositionCommand] },
            RemoteAction::Seek(0.0),
        );
        let previous = install(
            unsafe { msg_send![&*center, previousTrackCommand] },
            RemoteAction::Previous,
        );
        let next = install(
            unsafe { msg_send![&*center, nextTrackCommand] },
            RemoteAction::Next,
        );
        Self {
            info,
            seek,
            previous,
            next,
            _transport: [play, pause, toggle],
            events,
            availability: None,
            published: None,
        }
    }

    pub fn actions(&self) -> impl Iterator<Item = RemoteAction> + '_ {
        self.events.try_iter()
    }

    pub fn publish(
        &mut self,
        title: &str,
        elapsed: f64,
        duration: f64,
        rate: f64,
        ended: bool,
        queue: &super::watch::Queue,
    ) {
        let available = (
            duration > 0.0,
            queue.previous.is_some(),
            queue.next.is_some(),
        );
        if self.availability != Some(available) {
            self.seek.enable(available.0);
            self.previous.enable(available.1);
            self.next.enable(available.2);
            self.availability = Some(available);
        }
        let playback_state = if ended {
            3usize
        } else if rate == 0.0 {
            2
        } else {
            1
        };
        let bucket = (elapsed * if rate == 0.0 { 1000.0 } else { 1.0 }).floor() as i64;
        let published = (bucket, duration, rate, playback_state);
        if self.published == Some(published) {
            return;
        }
        self.published = Some(published);
        let title = NSString::from_str(title);
        let elapsed = NSNumber::numberWithDouble(elapsed);
        let duration = NSNumber::numberWithDouble(duration);
        let rate = NSNumber::numberWithDouble(rate);
        let video = NSNumber::numberWithUnsignedInteger(2);
        unsafe {
            let metadata = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    MPMediaItemPropertyTitle,
                    MPMediaItemPropertyPlaybackDuration,
                    MPNowPlayingInfoPropertyElapsedPlaybackTime,
                    MPNowPlayingInfoPropertyPlaybackRate,
                    MPNowPlayingInfoPropertyMediaType,
                ],
                &[
                    title.as_ref(),
                    duration.as_ref(),
                    elapsed.as_ref(),
                    rate.as_ref(),
                    video.as_ref(),
                ],
            );
            let _: () = msg_send![&*self.info, setNowPlayingInfo: &*metadata];
            let _: () = msg_send![&*self.info, setPlaybackState: playback_state];
        }
    }
}

impl Drop for NowPlaying {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![&*self.info, setPlaybackState: 3usize];
            let _: () = msg_send![&*self.info, setNowPlayingInfo: std::ptr::null::<AnyObject>()];
        }
    }
}
