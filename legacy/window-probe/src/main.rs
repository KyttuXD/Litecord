//! Opens a window on an old Mac and shows a picture that is drawn on the CPU and uploaded
//! as a texture on every frame. That is the path a legacy Litecord interface would take
//! (a software-rendered interface blitted through OpenGL 2.1). Prints what happened, counts
//! mouse and keyboard events, and closes by itself after a few seconds.

use miniquad::*;
use std::time::{Duration, Instant};

const W: usize = 480;
const H: usize = 300;
const RUN_FOR: Duration = Duration::from_secs(6);

#[repr(C)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

struct Stage {
    ctx: Box<dyn RenderingBackend>,
    pipeline: Pipeline,
    bindings: Bindings,
    texture: TextureId,
    pixels: Vec<u8>,
    frame: u32,
    started: Instant,
    mouse_events: u32,
    key_events: u32,
}

impl Stage {
    fn new() -> Stage {
        let mut ctx: Box<dyn RenderingBackend> = window::new_rendering_backend();
        let vertices: [Vertex; 4] = [
            Vertex { pos: [-1.0, -1.0], uv: [0.0, 1.0] },
            Vertex { pos: [1.0, -1.0], uv: [1.0, 1.0] },
            Vertex { pos: [1.0, 1.0], uv: [1.0, 0.0] },
            Vertex { pos: [-1.0, 1.0], uv: [0.0, 0.0] },
        ];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let vertex_buffer = ctx.new_buffer(
            BufferType::VertexBuffer,
            BufferUsage::Immutable,
            BufferSource::slice(&vertices),
        );
        let index_buffer = ctx.new_buffer(
            BufferType::IndexBuffer,
            BufferUsage::Immutable,
            BufferSource::slice(&indices),
        );
        let pixels = vec![0u8; W * H * 4];
        let texture = ctx.new_texture_from_rgba8(W as u16, H as u16, &pixels);
        let bindings = Bindings {
            vertex_buffers: vec![vertex_buffer],
            index_buffer,
            images: vec![texture],
        };
        let shader = ctx
            .new_shader(
                ShaderSource::Glsl { vertex: VERTEX, fragment: FRAGMENT },
                ShaderMeta {
                    images: vec!["tex".to_string()],
                    uniforms: UniformBlockLayout { uniforms: vec![] },
                },
            )
            .unwrap_or_else(|e| {
                println!("FAIL  shader: {e:?}");
                std::process::exit(1);
            });
        let pipeline = ctx.new_pipeline(
            &[BufferLayout::default()],
            &[
                VertexAttribute::new("in_pos", VertexFormat::Float2),
                VertexAttribute::new("in_uv", VertexFormat::Float2),
            ],
            shader,
            PipelineParams::default(),
        );
        println!("PASS  window and OpenGL context created, shader compiled");
        Stage {
            ctx,
            pipeline,
            bindings,
            texture,
            pixels,
            frame: 0,
            started: Instant::now(),
            mouse_events: 0,
            key_events: 0,
        }
    }

    /// The CPU-drawn picture: Litecord's dark background, a moving accent bar and a grid.
    fn paint(&mut self) {
        let bar = (self.frame as usize * 3) % W;
        for y in 0..H {
            for x in 0..W {
                let i = (y * W + x) * 4;
                let grid = x % 40 == 0 || y % 40 == 0;
                let in_bar = x >= bar && x < bar + 24 && y > 120 && y < 180;
                let (r, g, b) = if in_bar {
                    (0x58, 0x65, 0xF2)
                } else if grid {
                    (0x2B, 0x2D, 0x31)
                } else {
                    (0x11, 0x12, 0x14)
                };
                self.pixels[i] = r;
                self.pixels[i + 1] = g;
                self.pixels[i + 2] = b;
                self.pixels[i + 3] = 0xFF;
            }
        }
    }
}

impl EventHandler for Stage {
    fn update(&mut self) {
        if self.started.elapsed() >= RUN_FOR {
            let secs = self.started.elapsed().as_secs_f32();
            println!(
                "PASS  {} frames in {:.1} s ({:.0} per second), {} mouse events, {} key events",
                self.frame,
                secs,
                self.frame as f32 / secs,
                self.mouse_events,
                self.key_events
            );
            println!("RESULT: the window test finished");
            std::process::exit(0);
        }
    }

    fn draw(&mut self) {
        self.paint();
        self.ctx.texture_update(self.texture, &self.pixels);
        self.ctx.begin_default_pass(PassAction::clear_color(0.0, 0.0, 0.0, 1.0));
        self.ctx.apply_pipeline(&self.pipeline);
        self.ctx.apply_bindings(&self.bindings);
        self.ctx.draw(0, 6, 1);
        self.ctx.end_render_pass();
        self.ctx.commit_frame();
        self.frame += 1;
        if self.frame == 1 {
            let (w, h) = window::screen_size();
            println!("PASS  first frame drawn, window is {w} x {h}");
        }
    }

    fn mouse_motion_event(&mut self, _x: f32, _y: f32) {
        self.mouse_events += 1;
    }

    fn mouse_button_down_event(&mut self, _button: MouseButton, _x: f32, _y: f32) {
        self.mouse_events += 1;
    }

    fn char_event(&mut self, _character: char, _keymods: KeyMods, _repeat: bool) {
        self.key_events += 1;
    }
}

fn main() {
    println!("litecord-legacy-window-probe {}", env!("CARGO_PKG_VERSION"));
    miniquad::start(
        conf::Conf {
            window_title: "Litecord legacy window test".to_string(),
            window_width: 960,
            window_height: 600,
            ..Default::default()
        },
        || Box::new(Stage::new()),
    );
}

const VERTEX: &str = r#"#version 100
attribute vec2 in_pos;
attribute vec2 in_uv;
varying lowp vec2 uv;
void main() {
    gl_Position = vec4(in_pos, 0.0, 1.0);
    uv = in_uv;
}"#;

const FRAGMENT: &str = r#"#version 100
varying lowp vec2 uv;
uniform sampler2D tex;
void main() {
    gl_FragColor = texture2D(tex, uv);
}"#;
