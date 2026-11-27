use anyhow::{Result, anyhow, bail};
use eframe::{
    egui::{PaintCallbackInfo, Rect},
    glow::{self, HasContext},
};

#[derive(Clone, Copy)]
pub struct Glass {
    pub rect: Rect,
    pub radius: f32,
    pub opacity: f32,
    pub frosted: bool,
    /// Native SF Symbol for a circular media control.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub symbol: Option<&'static std::ffi::CStr>,
}

pub(super) struct GlassProgram {
    program: glow::Program,
    vao: glow::VertexArray,
    video: Option<glow::UniformLocation>,
    bounds: Option<glow::UniformLocation>,
    material: Option<glow::UniformLocation>,
    corners: Option<glow::UniformLocation>,
}

struct Material {
    corners: [f32; 4],
    opacity: f32,
    effect: f32,
}

const VERTEX: &str = r#"
void main() {
    vec2 point = vec2(gl_VertexID == 1 ? 3.0 : -1.0, gl_VertexID == 2 ? 3.0 : -1.0);
    gl_Position = vec4(point, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
uniform sampler2D u_frame;
uniform vec4 u_video;
uniform vec4 u_bounds;
uniform vec3 u_material;
uniform vec4 u_corners;
out vec4 color;

void main() {
    float scale = u_material.x;
    vec2 point = gl_FragCoord.xy - u_bounds.xy;
    vec2 row = point.y > 0.0 ? u_corners.xy : u_corners.zw;
    float radius = min(point.x < 0.0 ? row.x : row.y, min(u_bounds.z, u_bounds.w));
    vec2 rounded = abs(point) - u_bounds.zw + radius;
    float edge = length(max(rounded, 0.0)) + min(max(rounded.x, rounded.y), 0.0) - radius;
    float distance = edge / scale;
    float coverage = 1.0 - smoothstep(-0.5, 0.5, edge);
    if (coverage <= 0.0) discard;

    if (u_material.z < 0.0) {
        vec2 uv = (gl_FragCoord.xy - u_video.xy) / u_video.zw;
        color = vec4(texture(u_frame, uv).rgb * coverage, coverage);
        return;
    }

    // Bend the video along the edge of the glass, including the rounded corners.
    vec2 normal = max(rounded, 0.0);
    if (normal.x + normal.y < 0.001) {
        normal = rounded.x > rounded.y ? vec2(1.0, 0.0) : vec2(0.0, 1.0);
    }
    normal = normalize(normal) * sign(point);
    float bevel = smoothstep(-9.0, 0.0, distance);
    vec2 lens = normal * bevel * bevel * 10.0 * scale;
    vec2 uv = (gl_FragCoord.xy - u_video.xy - lens) / u_video.zw;
    vec2 step_uv = mix(mix(2.2, 1.1, bevel), 8.0, u_material.z) * scale / u_video.zw;

    // Rotate the sampling grid to soften strong horizontal and vertical edges.
    mat2 rotation = mat2(0.92, 0.39, -0.39, 0.92);
    vec3 blur = vec3(0.0);
    for (int y = 0; y < 4; y++) {
        for (int x = 0; x < 4; x++) {
            vec2 offset = rotation * (vec2(float(x), float(y)) - 1.5);
            float weight_x = x == 0 || x == 3 ? 1.0 : 2.0;
            float weight_y = y == 0 || y == 3 ? 1.0 : 2.0;
            blur += texture(u_frame, uv + step_uv * offset).rgb * weight_x * weight_y;
        }
    }
    vec3 detail = texture(u_frame, uv).rgb;
    if (bevel > 0.02) {
        vec2 dispersion = normal * (bevel * 0.75 * scale) / u_video.zw;
        detail.r = texture(u_frame, uv + dispersion).r;
        detail.b = texture(u_frame, uv - dispersion).b;
    }
    vec3 transmitted = mix(blur / 36.0, detail, mix(0.55, 0.78, bevel) * (1.0 - u_material.z));

    // Preserve color and soften only the brightest backgrounds behind the white controls.
    float brightness = dot(transmitted, vec3(0.2126, 0.7152, 0.0722));
    float dimming = smoothstep(0.48, 0.95, brightness);
    vec3 glass = mix(transmitted, vec3(1.0), 0.035 * (1.0 - dimming));
    glass *= mix(1.0, 0.63, dimming);

    float direction = dot(normal, vec2(-0.38, 0.925));
    float light = pow(max(direction, 0.0), 2.0);
    float dark = pow(max(-direction, 0.0), 2.0);
    float reflection = 0.07 + light * 0.64 + dark * 0.21;
    float rim = exp(min(distance, 0.0) / 1.05);
    glass = mix(glass, vec3(1.0), reflection * rim);
    float alpha = coverage * u_material.y;
    color = vec4(glass * alpha, alpha);
}
"#;

impl GlassProgram {
    pub fn new(gl: &glow::Context) -> Result<Self> {
        let header = if gl.version().is_embedded {
            "#version 300 es\nprecision highp float;\n"
        } else {
            "#version 150\n"
        };
        unsafe {
            let program = gl.create_program().map_err(|error| anyhow!(error))?;
            let result = Self::link(gl, program, header)
                .and_then(|()| gl.create_vertex_array().map_err(|error| anyhow!(error)));
            match result {
                Ok(vao) => Ok(Self {
                    program,
                    vao,
                    video: gl.get_uniform_location(program, "u_video"),
                    bounds: gl.get_uniform_location(program, "u_bounds"),
                    material: gl.get_uniform_location(program, "u_material"),
                    corners: gl.get_uniform_location(program, "u_corners"),
                }),
                Err(error) => {
                    gl.delete_program(program);
                    Err(error)
                }
            }
        }
    }

    fn link(gl: &glow::Context, program: glow::Program, header: &str) -> Result<()> {
        unsafe {
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX),
                (glow::FRAGMENT_SHADER, FRAGMENT),
            ] {
                let shader = gl.create_shader(kind).map_err(|error| anyhow!(error))?;
                gl.shader_source(shader, &format!("{header}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    let error = gl.get_shader_info_log(shader);
                    gl.delete_shader(shader);
                    bail!(error);
                }
                gl.attach_shader(program, shader);
                gl.delete_shader(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                bail!(gl.get_program_info_log(program));
            }
        }
        Ok(())
    }

    pub fn draw(
        &self,
        gl: &glow::Context,
        info: &PaintCallbackInfo,
        glass: Glass,
        mask: super::VideoMask,
        texture: glow::Texture,
        video: [i32; 4],
    ) {
        let edge = mask.corners(glass.rect);
        self.draw_region(
            gl,
            info,
            glass.rect,
            Material {
                corners: [edge.nw, edge.ne, edge.sw, edge.se]
                    .map(|r| f32::from(r).max(glass.radius)),
                opacity: glass.opacity,
                effect: if glass.frosted { 1. } else { 0. },
            },
            texture,
            video,
        );
    }

    pub fn draw_video(
        &self,
        gl: &glow::Context,
        info: &PaintCallbackInfo,
        mask: super::VideoMask,
        texture: glow::Texture,
        video: [i32; 4],
    ) {
        self.draw_region(
            gl,
            info,
            mask.frame,
            Material {
                corners: [f32::from(mask.radius); 4],
                opacity: 1.,
                effect: -1.,
            },
            texture,
            video,
        );
    }

    fn draw_region(
        &self,
        gl: &glow::Context,
        info: &PaintCallbackInfo,
        rect: Rect,
        material: Material,
        texture: glow::Texture,
        video: [i32; 4],
    ) {
        let clip = PaintCallbackInfo {
            clip_rect: info.clip_rect.intersect(info.viewport).intersect(rect),
            ..*info
        }
        .clip_rect_in_pixels();
        let scale = info.pixels_per_point;
        let [x, y, width, height] = video;
        let center = rect.center();
        let half_size = rect.size() * (0.5 * scale);
        unsafe {
            gl.viewport(x, y, width, height);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            gl.enable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.uniform_4_f32(
                self.video.as_ref(),
                x as f32,
                y as f32,
                width as f32,
                height as f32,
            );
            gl.uniform_4_f32(
                self.bounds.as_ref(),
                center.x * scale,
                info.screen_size_px[1] as f32 - center.y * scale,
                half_size.x,
                half_size.y,
            );
            let [nw, ne, sw, se] = material.corners.map(|r| r * scale);
            gl.uniform_4_f32(self.corners.as_ref(), nw, ne, sw, se);
            gl.uniform_3_f32(
                self.material.as_ref(),
                scale,
                material.opacity,
                material.effect,
            );
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
    }

    pub fn delete(self, gl: &glow::Context) {
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_program(self.program);
        }
    }
}
