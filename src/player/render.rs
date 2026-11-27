use std::{
    cell::OnceCell,
    ffi::{CStr, c_char, c_int, c_void},
    ptr::{self, NonNull},
    sync::{Arc, Mutex},
};

use anyhow::{Context as _, Result, bail, ensure};
use crossbeam_channel::Sender;
use eframe::{
    egui,
    egui_glow::Painter,
    glow::{self, HasContext},
};

use super::{Control, Core, GlProc, Glass, Load, Playback, VideoMask, ffi, glass::GlassProgram};

pub(super) struct Renderer {
    core: Arc<Core>,
    loader: Box<GlProc>,
    gui: Box<egui::Context>,
    commands: Sender<Control>,
    state: Arc<Mutex<Playback>>,
    context: Option<NonNull<c_void>>,
    buffer: Option<Buffer>,
    glass: OnceCell<Option<GlassProgram>>,
    pending: Option<Load>,
    failed: bool,
}

// The GL resources and render context are touched only by eframe's render thread.
// The shared value is locked because egui's paint callback must be Send + Sync.
unsafe impl Send for Renderer {}

impl Renderer {
    pub fn new(
        core: Arc<Core>,
        loader: GlProc,
        commands: Sender<Control>,
        state: Arc<Mutex<Playback>>,
        gui: egui::Context,
    ) -> Self {
        Self {
            core,
            loader: Box::new(loader),
            gui: Box::new(gui),
            commands,
            state,
            context: None,
            buffer: None,
            glass: OnceCell::new(),
            pending: None,
            failed: false,
        }
    }

    pub fn load(&mut self, load: Load) {
        if self.context.is_some() {
            let _ = self.commands.send(Control::Load(load));
        } else {
            self.pending = Some(load);
        }
    }

    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    pub fn draw(
        &mut self,
        info: egui::PaintCallbackInfo,
        painter: &Painter,
        glass: &[Glass],
        load_id: u64,
        mask: VideoMask,
    ) {
        if self.failed {
            return;
        }
        let present = {
            let playback = self.state.lock().unwrap();
            if playback.load_id != load_id {
                return;
            }
            playback.frame_ready
        };
        let gl = painter.gl();
        let result = self.paint(info, painter, glass, present, mask);
        // mpv and the offscreen blit change the framebuffer and scissor state.
        // egui reestablishes its remaining GL state after each callback.
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo()) };
        if let Err(error) = result {
            self.failed = true;
            self.state.lock().unwrap().error =
                Some(format!("Could not render the video: {error:#}"));
            let _ = self.commands.send(Control::Stop);
            self.gui.request_repaint();
        }
    }

    fn create_context(&mut self) -> Result<NonNull<c_void>> {
        let mut init = ffi::GlInit {
            lookup: get_proc,
            context: ptr::from_mut(&mut *self.loader).cast(),
        };
        let mut args = [
            ffi::RenderParam::new(ffi::RENDER_API, c"opengl".as_ptr().cast_mut()),
            ffi::RenderParam::new(ffi::RENDER_GL_INIT, &mut init),
            ffi::RenderParam::END,
        ];
        let mut context = ptr::null_mut();
        self.core.check(unsafe {
            (self.core.api.mpv_render_context_create)(
                &mut context,
                self.core.handle.as_ptr(),
                args.as_mut_ptr(),
            )
        })?;
        let context = NonNull::new(context).context("mpv returned an empty render context")?;
        unsafe {
            (self.core.api.mpv_render_context_set_update_callback)(
                context.as_ptr(),
                Some(next_frame),
                ptr::from_mut(&mut *self.gui).cast(),
            );
        }
        self.context = Some(context);
        if let Some(load) = self.pending.take() {
            let _ = self.commands.send(Control::Load(load));
        }
        Ok(context)
    }

    fn paint(
        &mut self,
        info: egui::PaintCallbackInfo,
        painter: &Painter,
        glass: &[Glass],
        present: bool,
        mask: VideoMask,
    ) -> Result<()> {
        if !info.viewport.intersects(info.clip_rect) {
            return Ok(());
        }
        // Keep the render size stable when a scroll area moves part of the video
        // off-screen. The framebuffer blit is clipped separately below.
        let pixel = |point: f32| (point * info.pixels_per_point).round() as i32;
        let left = pixel(info.viewport.left());
        let right = pixel(info.viewport.right());
        let screen_height = info.screen_size_px[1] as i32;
        let bottom = screen_height - pixel(info.viewport.bottom());
        let top = screen_height - pixel(info.viewport.top());
        let size = (right - left, top - bottom);
        if size.0 <= 0 || size.1 <= 0 {
            return Ok(());
        }
        let gl = painter.gl();
        // The render API assumes default GL enable states. egui's painter
        // enters the callback with blending and clipping enabled.
        unsafe {
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
        let context = match self.context {
            Some(context) => context,
            None => self.create_context()?,
        };
        if self
            .buffer
            .as_ref()
            .is_none_or(|buffer| buffer.size != size)
        {
            if let Some(old) = self.buffer.take() {
                old.delete(gl);
            }
            self.buffer = Some(Buffer::new(gl, size)?);
        }
        let buffer = self.buffer.as_ref().context("Missing video framebuffer")?;
        let mut fbo = ffi::Fbo {
            id: buffer.id.0.get() as c_int,
            w: size.0,
            h: size.1,
            format: glow::RGBA8 as c_int,
        };
        let mut flip = 1 as c_int;
        let mut block = 0 as c_int;
        let mut params = [
            ffi::RenderParam::new(ffi::RENDER_GL_FBO, &mut fbo),
            ffi::RenderParam::new(ffi::RENDER_FLIP_Y, &mut flip),
            ffi::RenderParam::new(ffi::RENDER_BLOCK, &mut block),
            ffi::RenderParam::END,
        ];
        self.core.check(unsafe {
            (self.core.api.mpv_render_context_render)(context.as_ptr(), params.as_mut_ptr())
        })?;
        // Keep servicing mpv while it primes the new file, but the framebuffer may
        // still contain the preceding movie until playback restarts for this load.
        if !present {
            return Ok(());
        }

        let clip = info.clip_rect_in_pixels();
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(buffer.id));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, painter.intermediate_fbo());

            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
        }
        if mask.radius > 0 {
            let program = self
                .glass
                .get_or_init(|| GlassProgram::new(gl).ok())
                .as_ref()
                .context("Could not initialize the rounded video shader")?;
            program.draw_video(
                gl,
                &info,
                mask,
                buffer.texture,
                [left, bottom, size.0, size.1],
            );
        } else {
            unsafe {
                gl.blit_framebuffer(
                    0,
                    0,
                    size.0,
                    size.1,
                    left,
                    bottom,
                    right,
                    top,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
            }
        }
        for &glass in glass {
            if info
                .viewport
                .intersect(info.clip_rect)
                .intersects(glass.rect)
                && let Some(program) = self.glass.get_or_init(|| GlassProgram::new(gl).ok())
            {
                program.draw(
                    gl,
                    &info,
                    glass,
                    mask,
                    buffer.texture,
                    [left, bottom, size.0, size.1],
                );
            }
        }
        Ok(())
    }

    pub fn destroy(&mut self, gl: Option<&glow::Context>) {
        if let Some(context) = self.context.take() {
            unsafe {
                (self.core.api.mpv_render_context_set_update_callback)(
                    context.as_ptr(),
                    None,
                    ptr::null_mut(),
                );
                (self.core.api.mpv_render_context_free)(context.as_ptr());
            }
        }
        if let Some(buffer) = self.buffer.take()
            && let Some(gl) = gl
        {
            buffer.delete(gl);
        }
        if let Some(program) = self.glass.take().flatten()
            && let Some(gl) = gl
        {
            program.delete(gl);
        }
    }
}

unsafe extern "C" fn get_proc(data: *mut c_void, name: *const c_char) -> *mut c_void {
    unsafe { (&*data.cast::<GlProc>())(CStr::from_ptr(name)).cast_mut() }
}

unsafe extern "C" fn next_frame(data: *mut c_void) {
    unsafe { (&*data.cast::<egui::Context>()).request_repaint_of(egui::ViewportId::ROOT) };
}

struct Buffer {
    id: glow::Framebuffer,
    texture: glow::Texture,
    size: (i32, i32),
}

impl Buffer {
    fn new(gl: &glow::Context, size: (i32, i32)) -> Result<Self> {
        unsafe {
            let max = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE);
            ensure!(
                size.0 <= max && size.1 <= max,
                "Video panel exceeds the GPU texture limit"
            );
            let id = gl.create_framebuffer().map_err(anyhow::Error::msg)?;
            let texture = match gl.create_texture() {
                Ok(texture) => texture,
                Err(error) => {
                    gl.delete_framebuffer(id);
                    bail!("{error}");
                }
            };
            let buffer = Self { id, texture, size };
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            for axis in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, axis, glow::CLAMP_TO_EDGE as i32);
            }
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size.0,
                size.1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(id));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                buffer.delete(gl);
                bail!("The GPU did not accept the playback framebuffer");
            }
            Ok(buffer)
        }
    }

    fn delete(self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.id);
            gl.delete_texture(self.texture);
        }
    }
}
