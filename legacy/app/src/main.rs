//! Litecord legacy edition: a small window for old Intel Macs (OS X 10.5 - 10.11) on top of
//! Litecord's own network code. Text chat only for now. The interface is drawn on the CPU
//! into one picture and shown through OpenGL 2.1 (miniquad with the old-Mac changes of
//! TheLastRoute), the path proven on a Mac Pro 3,1 with OS X 10.6.8.

mod backend;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use backend::{Action, Shared, SharedRef};
use miniquad::*;
use tokio::sync::mpsc::UnboundedSender;

const BG: [u8; 3] = [0x31, 0x33, 0x38];
const SIDEBAR: [u8; 3] = [0x1E, 0x1F, 0x22];
const PANEL: [u8; 3] = [0x2B, 0x2D, 0x31];
const INPUT: [u8; 3] = [0x38, 0x3A, 0x40];
const ACCENT: [u8; 3] = [0x58, 0x65, 0xF2];
const TEXT: [u8; 3] = [0xDB, 0xDE, 0xE1];
const MUTED: [u8; 3] = [0x94, 0x9B, 0xA4];
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

const GUILDS_W: i32 = 190;
const CHANNELS_W: i32 = 220;
const TOP_H: i32 = 30;
const INPUT_H: i32 = 40;
const ROW_H: i32 = 26;
const LINE_H: i32 = 20;
const FONT: f32 = 15.0;

/// The picture the interface is drawn into, with the text rasterizer.
struct Canvas {
    w: i32,
    h: i32,
    px: Vec<u8>,
    font: fontdue::Font,
    glyphs: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

impl Canvas {
    fn new(w: i32, h: i32) -> Canvas {
        let font = fontdue::Font::from_bytes(
            include_bytes!("../../../assets/fonts/Inter-Variable.ttf") as &[u8],
            fontdue::FontSettings::default(),
        )
        .expect("the bundled font is readable");
        Canvas { w, h, px: vec![0; (w * h * 4) as usize], font, glyphs: HashMap::new() }
    }

    fn resize(&mut self, w: i32, h: i32) {
        self.w = w.max(1);
        self.h = h.max(1);
        self.px = vec![0; (self.w * self.h * 4) as usize];
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 3]) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.w), (y + h).min(self.h));
        for yy in y0..y1 {
            let row = (yy * self.w) as usize * 4;
            for xx in x0..x1 {
                let i = row + xx as usize * 4;
                self.px[i] = c[0];
                self.px[i + 1] = c[1];
                self.px[i + 2] = c[2];
                self.px[i + 3] = 0xFF;
            }
        }
    }

    fn glyph(&mut self, ch: char, size: f32) -> &(fontdue::Metrics, Vec<u8>) {
        let key = (ch, size as u32);
        let font = &self.font;
        self.glyphs.entry(key).or_insert_with(|| font.rasterize(ch, size))
    }

    fn text_width(&mut self, text: &str, size: f32) -> i32 {
        let mut w = 0.0;
        for ch in text.chars() {
            w += self.glyph(ch, size).0.advance_width;
        }
        w.ceil() as i32
    }

    /// Draws one line with its baseline at `y`, cut at `max_x`. Returns the x after the text.
    fn text(&mut self, x: i32, y: i32, text: &str, size: f32, c: [u8; 3], max_x: i32) -> i32 {
        let mut pen = x as f32;
        let (cw, chh) = (self.w, self.h);
        for ch in text.chars() {
            let (m, bitmap) = self.glyph(ch, size).clone();
            let gx = pen.round() as i32 + m.xmin;
            let gy = y - m.ymin - m.height as i32;
            if gx + m.width as i32 > max_x {
                break;
            }
            for row in 0..m.height as i32 {
                let yy = gy + row;
                if yy < 0 || yy >= chh {
                    continue;
                }
                for col in 0..m.width as i32 {
                    let xx = gx + col;
                    if xx < 0 || xx >= cw {
                        continue;
                    }
                    let a = bitmap[(row * m.width as i32 + col) as usize] as u32;
                    if a == 0 {
                        continue;
                    }
                    let i = ((yy * cw + xx) * 4) as usize;
                    for k in 0..3 {
                        let old = self.px[i + k] as u32;
                        self.px[i + k] = ((c[k] as u32 * a + old * (255 - a)) / 255) as u8;
                    }
                }
            }
            pen += m.advance_width;
        }
        pen.ceil() as i32
    }

    /// Breaks text into lines no wider than `max_w`.
    fn wrap(&mut self, text: &str, size: f32, max_w: i32) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            let mut line = String::new();
            let mut line_w = 0.0f32;
            for word in paragraph.split_inclusive(' ') {
                let word_w: f32 = word.chars().map(|ch| self.glyph(ch, size).0.advance_width).sum();
                if line_w + word_w > max_w as f32 && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    line_w = 0.0;
                }
                if word_w > max_w as f32 {
                    // A word wider than the pane (a long link): cut it letter by letter.
                    for ch in word.chars() {
                        let ch_w = self.glyph(ch, size).0.advance_width;
                        if line_w + ch_w > max_w as f32 && !line.is_empty() {
                            lines.push(std::mem::take(&mut line));
                            line_w = 0.0;
                        }
                        line.push(ch);
                        line_w += ch_w;
                    }
                } else {
                    line.push_str(word);
                    line_w += word_w;
                }
            }
            lines.push(line);
        }
        lines
    }
}

struct Stage {
    ctx: Box<dyn RenderingBackend>,
    pipeline: Pipeline,
    bindings: Bindings,
    canvas: Canvas,
    shared: SharedRef,
    actions: UnboundedSender<Action>,
    seen_version: u64,
    dirty: bool,
    mouse: (i32, i32),
    guild: usize,
    guild_scroll: i32,
    channel_scroll: i32,
    /// Lines scrolled up from the newest message.
    message_scroll: i32,
    input: String,
    /// Rows drawn last frame, for clicks: (y, index).
    guild_rows: Vec<(i32, usize)>,
    channel_rows: Vec<(i32, String)>,
    logout_x: i32,
}

#[repr(C)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

impl Stage {
    fn new(shared: SharedRef, actions: UnboundedSender<Action>) -> Stage {
        let mut ctx: Box<dyn RenderingBackend> = window::new_rendering_backend();
        let (w, h) = window::screen_size();
        let canvas = Canvas::new(w as i32, h as i32);
        let vertices: [Vertex; 4] = [
            Vertex { pos: [-1.0, -1.0], uv: [0.0, 1.0] },
            Vertex { pos: [1.0, -1.0], uv: [1.0, 1.0] },
            Vertex { pos: [1.0, 1.0], uv: [1.0, 0.0] },
            Vertex { pos: [-1.0, 1.0], uv: [0.0, 0.0] },
        ];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let vertex_buffer =
            ctx.new_buffer(BufferType::VertexBuffer, BufferUsage::Immutable, BufferSource::slice(&vertices));
        let index_buffer =
            ctx.new_buffer(BufferType::IndexBuffer, BufferUsage::Immutable, BufferSource::slice(&indices));
        let texture = ctx.new_texture_from_rgba8(canvas.w as u16, canvas.h as u16, &canvas.px);
        let bindings = Bindings { vertex_buffers: vec![vertex_buffer], index_buffer, images: vec![texture] };
        let shader = ctx
            .new_shader(
                ShaderSource::Glsl { vertex: VERTEX, fragment: FRAGMENT },
                ShaderMeta { images: vec!["tex".to_string()], uniforms: UniformBlockLayout { uniforms: vec![] } },
            )
            .expect("the blit shader compiles");
        let pipeline = ctx.new_pipeline(
            &[BufferLayout::default()],
            &[
                VertexAttribute::new("in_pos", VertexFormat::Float2),
                VertexAttribute::new("in_uv", VertexFormat::Float2),
            ],
            shader,
            PipelineParams::default(),
        );
        log::info!("window ready: {} x {}", canvas.w, canvas.h);
        Stage {
            ctx,
            pipeline,
            bindings,
            canvas,
            shared,
            actions,
            seen_version: u64::MAX,
            dirty: true,
            mouse: (0, 0),
            guild: 0,
            guild_scroll: 0,
            channel_scroll: 0,
            message_scroll: 0,
            input: String::new(),
            guild_rows: Vec::new(),
            channel_rows: Vec::new(),
            logout_x: i32::MAX,
        }
    }

    fn paint(&mut self) {
        let shared = Arc::clone(&self.shared);
        let Ok(state) = shared.lock() else { return };
        self.seen_version = state.version;
        let (w, h) = (self.canvas.w, self.canvas.h);
        self.canvas.rect(0, 0, w, h, BG);
        if state.logged_in {
            self.paint_main(&state);
        } else {
            self.paint_login(&state);
        }
    }

    fn paint_login(&mut self, state: &Shared) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        self.guild_rows.clear();
        self.channel_rows.clear();
        self.logout_x = i32::MAX;
        let title = "Litecord (legacy edition)";
        let tw = self.canvas.text_width(title, 24.0);
        self.canvas.text((w - tw) / 2, 60, title, 24.0, WHITE, w);
        if let Some((rgba, qw, qh)) = &state.qr {
            let (qw, qh) = (*qw as i32, *qh as i32);
            // Whole-number scale, so the code stays sharp for the phone camera.
            let scale = ((h - 200) / qh.max(1)).min((w - 80) / qw.max(1)).max(1);
            let (dw, dh) = (qw * scale, qh * scale);
            let (ox, oy) = ((w - dw) / 2, 90);
            self.canvas.rect(ox - 8, oy - 8, dw + 16, dh + 16, WHITE);
            for y in 0..dh {
                for x in 0..dw {
                    let s = (((y / scale) * qw + x / scale) * 4) as usize;
                    let (xx, yy) = (ox + x, oy + y);
                    if xx < 0 || yy < 0 || xx >= w || yy >= h {
                        continue;
                    }
                    let d = ((yy * w + xx) * 4) as usize;
                    self.canvas.px[d..d + 3].copy_from_slice(&rgba[s..s + 3]);
                    self.canvas.px[d + 3] = 0xFF;
                }
            }
        }
        let lines = self.canvas.wrap(&state.status, FONT, w - 80);
        for (i, line) in lines.iter().enumerate() {
            let lw = self.canvas.text_width(line, FONT);
            self.canvas.text((w - lw) / 2, h - 70 + i as i32 * LINE_H, line, FONT, TEXT, w);
        }
    }

    fn paint_main(&mut self, state: &Shared) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        if self.guild >= state.guilds.len() {
            self.guild = 0;
        }

        // Servers.
        self.canvas.rect(0, TOP_H, GUILDS_W, h - TOP_H, SIDEBAR);
        self.guild_rows.clear();
        for (i, guild) in state.guilds.iter().enumerate() {
            let y = TOP_H + 6 + i as i32 * ROW_H - self.guild_scroll;
            if y < TOP_H - ROW_H || y > h {
                continue;
            }
            if i == self.guild {
                self.canvas.rect(4, y, GUILDS_W - 8, ROW_H - 2, ACCENT);
            }
            let color = if i == self.guild { WHITE } else { TEXT };
            self.canvas.text(12, y + 17, &guild.name, FONT, color, GUILDS_W - 8);
            self.guild_rows.push((y, i));
        }

        // Channels of the selected server: text channels can be opened, the rest is shown in grey.
        self.canvas.rect(GUILDS_W, TOP_H, CHANNELS_W, h - TOP_H, PANEL);
        self.channel_rows.clear();
        if let Some(guild) = state.guilds.get(self.guild) {
            let mut channels: Vec<_> = guild.channels.iter().collect();
            if guild.id != "@me" {
                channels.sort_by_key(|c| c.position);
            }
            for (i, channel) in channels.iter().enumerate() {
                let y = TOP_H + 6 + i as i32 * ROW_H - self.channel_scroll;
                if y < TOP_H - ROW_H || y > h {
                    continue;
                }
                let selected = channel.id == state.channel_id;
                if selected {
                    self.canvas.rect(GUILDS_W + 4, y, CHANNELS_W - 8, ROW_H - 2, INPUT);
                }
                let (label, color) = if channel.is_category {
                    (channel.name.to_uppercase(), MUTED)
                } else if channel.is_voice {
                    (format!("(voice) {}", channel.name), MUTED)
                } else if guild.id == "@me" {
                    (channel.name.clone(), if selected { WHITE } else { TEXT })
                } else {
                    (format!("# {}", channel.name), if selected { WHITE } else { TEXT })
                };
                self.canvas.text(GUILDS_W + 12, y + 17, &label, FONT, color, GUILDS_W + CHANNELS_W - 8);
                if !channel.is_category && !channel.is_voice {
                    self.channel_rows.push((y, channel.id.clone()));
                }
            }
        }

        // Messages, newest at the bottom.
        let mx = GUILDS_W + CHANNELS_W;
        let pane_w = w - mx - 24;
        let bottom = h - INPUT_H - 8;
        let mut lines: Vec<(String, [u8; 3])> = Vec::new();
        for message in &state.messages {
            lines.push((format!("{}   {}", message.author, message.time), ACCENT));
            for line in self.canvas.wrap(&message.text, FONT, pane_w.max(40)) {
                lines.push((line, TEXT));
            }
            lines.push((String::new(), TEXT));
        }
        let visible = ((bottom - TOP_H) / LINE_H).max(1);
        let max_scroll = (lines.len() as i32 - visible).max(0);
        self.message_scroll = self.message_scroll.clamp(0, max_scroll);
        let end = lines.len() as i32 - self.message_scroll;
        let start = (end - visible).max(0);
        for (row, (line, color)) in lines[start as usize..end as usize].iter().enumerate() {
            let y = bottom - (end - start - row as i32) * LINE_H + 15;
            self.canvas.text(mx + 12, y, line, FONT, *color, w - 8);
        }
        if state.channel_id.is_empty() {
            self.canvas.text(mx + 12, TOP_H + 40, "Choose a server, then a channel.", FONT, MUTED, w - 8);
        }

        // Text input.
        self.canvas.rect(mx + 8, h - INPUT_H, w - mx - 16, INPUT_H - 8, INPUT);
        let shown = if self.input.is_empty() && !state.channel_id.is_empty() {
            ("Type a message and press Return".to_string(), MUTED)
        } else {
            (format!("{}|", self.input), TEXT)
        };
        // Keep the end of a long line in view.
        let mut text = shown.0;
        while self.canvas.text_width(&text, FONT) > w - mx - 40 && text.chars().count() > 1 {
            text.remove(0);
        }
        self.canvas.text(mx + 16, h - INPUT_H + 21, &text, FONT, shown.1, w - 12);

        // Top bar: account, status, log out.
        self.canvas.rect(0, 0, w, TOP_H, SIDEBAR);
        let x = self.canvas.text(12, 20, &state.user, FONT, WHITE, w);
        let logout = "Log out";
        self.logout_x = w - self.canvas.text_width(logout, FONT) - 12;
        self.canvas.text(x + 16, 20, &state.status, FONT, MUTED, self.logout_x - 12);
        self.canvas.text(self.logout_x, 20, logout, FONT, MUTED, w);
    }

    fn click(&mut self, x: i32, y: i32) {
        if y < TOP_H {
            if x >= self.logout_x {
                let _ = self.actions.send(Action::LogOut);
            }
            return;
        }
        if x < GUILDS_W {
            if let Some((_, i)) = self.guild_rows.iter().find(|(ry, _)| y >= *ry && y < *ry + ROW_H) {
                self.guild = *i;
                self.channel_scroll = 0;
                self.dirty = true;
            }
        } else if x < GUILDS_W + CHANNELS_W {
            let hit = self.channel_rows.iter().find(|(ry, _)| y >= *ry && y < *ry + ROW_H).map(|(_, id)| id.clone());
            if let Some(channel_id) = hit {
                let guild_id = self
                    .shared
                    .lock()
                    .ok()
                    .and_then(|s| s.guilds.get(self.guild).map(|g| g.id.clone()))
                    .unwrap_or_default();
                self.message_scroll = 0;
                let _ = self.actions.send(Action::SelectChannel { guild_id, channel_id });
            }
        }
    }
}

impl EventHandler for Stage {
    fn update(&mut self) {
        if let Ok(state) = self.shared.lock() {
            if state.version != self.seen_version {
                self.dirty = true;
            }
        }
    }

    fn draw(&mut self) {
        if self.dirty {
            self.dirty = false;
            self.paint();
            self.ctx.texture_update(self.bindings.images[0], &self.canvas.px);
        }
        self.ctx.begin_default_pass(PassAction::clear_color(0.0, 0.0, 0.0, 1.0));
        self.ctx.apply_pipeline(&self.pipeline);
        self.ctx.apply_bindings(&self.bindings);
        self.ctx.draw(0, 6, 1);
        self.ctx.end_render_pass();
        self.ctx.commit_frame();
    }

    fn resize_event(&mut self, width: f32, height: f32) {
        self.canvas.resize(width as i32, height as i32);
        self.ctx.delete_texture(self.bindings.images[0]);
        self.bindings.images[0] =
            self.ctx.new_texture_from_rgba8(self.canvas.w as u16, self.canvas.h as u16, &self.canvas.px);
        self.dirty = true;
    }

    fn mouse_motion_event(&mut self, x: f32, y: f32) {
        self.mouse = (x as i32, y as i32);
    }

    fn mouse_button_down_event(&mut self, button: MouseButton, x: f32, y: f32) {
        if button == MouseButton::Left {
            self.click(x as i32, y as i32);
        }
    }

    fn mouse_wheel_event(&mut self, _x: f32, y: f32) {
        let step = if y > 0.0 { -1 } else if y < 0.0 { 1 } else { 0 };
        if self.mouse.0 < GUILDS_W {
            self.guild_scroll = (self.guild_scroll + step * ROW_H * 2).max(0);
        } else if self.mouse.0 < GUILDS_W + CHANNELS_W {
            self.channel_scroll = (self.channel_scroll + step * ROW_H * 2).max(0);
        } else {
            // Wheel up shows older messages.
            self.message_scroll = (self.message_scroll - step * 3).max(0);
        }
        self.dirty = true;
    }

    fn char_event(&mut self, character: char, keymods: KeyMods, _repeat: bool) {
        if character.is_control() || keymods.logo || keymods.ctrl {
            return;
        }
        // Private-use code points are how the system reports arrow and function keys.
        if ('\u{F700}'..='\u{F8FF}').contains(&character) {
            return;
        }
        self.input.push(character);
        self.dirty = true;
    }

    fn key_down_event(&mut self, keycode: KeyCode, _keymods: KeyMods, _repeat: bool) {
        match keycode {
            KeyCode::Backspace => {
                self.input.pop();
                self.dirty = true;
            }
            KeyCode::Enter | KeyCode::KpEnter => {
                let text = self.input.trim().to_string();
                let channel_id = self.shared.lock().map(|s| s.channel_id.clone()).unwrap_or_default();
                if !text.is_empty() && !channel_id.is_empty() {
                    let _ = self.actions.send(Action::Send { channel_id, text });
                    self.input.clear();
                    self.message_scroll = 0;
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }
}

/// Log lines go to the terminal and to `litecord-legacy.log` in the data folder.
struct Logger {
    file: Mutex<Option<std::fs::File>>,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{} {} {}", record.level(), record.target(), record.args());
        eprintln!("{line}");
        if let Ok(mut file) = self.file.lock() {
            if let Some(file) = file.as_mut() {
                use std::io::Write;
                let _ = writeln!(file, "{line}");
            }
        }
    }
    fn flush(&self) {}
}

fn main() {
    litecord_legacy_compat::init();
    let dir = backend::data_dir();
    let _ = std::env::set_current_dir(&dir);
    let file = std::fs::File::create(dir.join("litecord-legacy.log")).ok();
    let _ = log::set_boxed_logger(Box::new(Logger { file: Mutex::new(file) }));
    log::set_max_level(log::LevelFilter::Info);
    log::info!("litecord-legacy {} starting, data in {}", env!("CARGO_PKG_VERSION"), dir.display());

    let shared: SharedRef = Arc::new(Mutex::new(Shared::default()));
    let (action_tx, action_rx) = tokio::sync::mpsc::unbounded_channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the async runtime starts");
    runtime.spawn(backend::run(Arc::clone(&shared), action_rx));

    miniquad::start(
        conf::Conf {
            window_title: "Litecord (legacy)".to_string(),
            window_width: 1000,
            window_height: 640,
            window_resizable: true,
            ..Default::default()
        },
        move || Box::new(Stage::new(shared, action_tx)),
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
