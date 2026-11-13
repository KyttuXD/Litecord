//! Litecord legacy edition: a small window for old Intel Macs (OS X 10.5 - 10.11) on top of
//! Litecord's own network and voice code: text chat and voice channels. The interface is drawn on the CPU
//! into one picture and shown through OpenGL 2.1 (miniquad with the old-Mac changes of
//! TheLastRoute), the path proven on a Mac Pro 3,1 with OS X 10.6.8.

mod backend;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use backend::{Action, CodeKind, LoginStage, Shared, SharedRef};
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
    /// (y, channel id, is voice, name).
    channel_rows: Vec<(i32, String, bool, String)>,
    logout_x: i32,
    /// Login form: the three text fields, which one has the keyboard, and what was drawn
    /// last frame for clicks: fields as (index, x, y, w, h), buttons as (label, x0, x1, y).
    logged_in: bool,
    login_fields: [String; 3],
    login_focus: usize,
    login_field_rects: Vec<(usize, i32, i32, i32, i32)>,
    login_buttons: Vec<(&'static str, i32, i32, i32)>,
    /// Voice bar buttons drawn last frame: (x from, x to) of Mute and Leave, and the bar's top.
    voice_bar: Option<(i32, (i32, i32), (i32, i32))>,
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
            logged_in: false,
            login_fields: [String::new(), String::new(), String::new()],
            login_focus: 0,
            login_field_rects: Vec::new(),
            login_buttons: Vec::new(),
            voice_bar: None,
        }
    }

    fn paint(&mut self) {
        let shared = Arc::clone(&self.shared);
        let Ok(state) = shared.lock() else { return };
        self.seen_version = state.version;
        let (w, h) = (self.canvas.w, self.canvas.h);
        self.canvas.rect(0, 0, w, h, BG);
        if state.logged_in != self.logged_in {
            self.logged_in = state.logged_in;
            // Nothing typed at the login screen stays in memory once it is left.
            self.login_fields = [String::new(), String::new(), String::new()];
            self.login_focus = 0;
        }
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
        self.voice_bar = None;
        self.login_field_rects.clear();
        self.login_buttons.clear();
        let title = "Litecord (legacy edition)";
        let tw = self.canvas.text_width(title, 24.0);
        self.canvas.text((w - tw) / 2, 50, title, 24.0, WHITE, w);

        // Left half: the QR code.
        let half = w / 2;
        if let Some((rgba, qw, qh)) = &state.qr {
            let (qw, qh) = (*qw as i32, *qh as i32);
            // Whole-number scale, so the code stays sharp for the phone camera.
            let scale = ((h - 230) / qh.max(1)).min((half - 80) / qw.max(1)).max(1);
            let (dw, dh) = (qw * scale, qh * scale);
            let (ox, oy) = ((half - dw) / 2, 100);
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

        // Right half: email and password, or the code of the second step.
        let fx = half + 30;
        let fw = (w - fx - 50).clamp(160, 360);
        let mut y = 110;
        let field = |stage: &mut Stage, index: usize, label: &str, hidden: bool, y: i32| {
            stage.canvas.text(fx, y, label, FONT, MUTED, w);
            let focused = stage.login_focus == index;
            stage.canvas.rect(fx - 1, y + 7, fw + 2, 32, if focused { ACCENT } else { PANEL });
            stage.canvas.rect(fx, y + 8, fw, 30, INPUT);
            let mut shown: String = if hidden {
                "*".repeat(stage.login_fields[index].chars().count())
            } else {
                stage.login_fields[index].clone()
            };
            if focused {
                shown.push('|');
            }
            while stage.canvas.text_width(&shown, FONT) > fw - 16 && shown.chars().count() > 1 {
                shown.remove(0);
            }
            stage.canvas.text(fx + 8, y + 29, &shown, FONT, TEXT, fx + fw);
            stage.login_field_rects.push((index, fx, y + 8, fw, 30));
        };
        let button = |stage: &mut Stage, label: &'static str, x: i32, y: i32, primary: bool| -> i32 {
            let x1 = stage.canvas.text(x, y, label, FONT, if primary { ACCENT } else { TEXT }, w);
            stage.login_buttons.push((label, x, x1, y));
            x1
        };
        if state.login_stage == LoginStage::Code {
            if self.login_focus != 2 {
                self.login_focus = 2;
            }
            let label = if state.login_code_kind == CodeKind::Backup { "Backup code" } else { "6-digit code" };
            field(self, 2, label, false, y);
            y += 70;
            let x = button(self, "Verify", fx, y, true);
            button(self, "Cancel", x + 24, y, false);
            y += 30;
            let mut x = fx;
            if state.login_has_sms {
                x = button(self, "Text me a code", x, y, false) + 24;
            }
            if state.login_code_kind == CodeKind::Backup {
                button(self, "Use a 6-digit code", x, y, false);
            } else {
                button(self, "Use a backup code", x, y, false);
            }
            y += 20;
        } else {
            if self.login_focus > 1 {
                self.login_focus = 0;
            }
            field(self, 0, "Email or phone number", false, y);
            y += 66;
            field(self, 1, "Password", true, y);
            y += 70;
            if state.login_stage == LoginStage::Busy {
                self.canvas.text(fx, y, "Logging in...", FONT, MUTED, w);
            } else {
                button(self, "Log in", fx, y, true);
            }
            y += 20;
        }
        let lines = self.canvas.wrap(&state.login_message, FONT, fw);
        for (i, line) in lines.iter().enumerate() {
            self.canvas.text(fx, y + 20 + i as i32 * LINE_H, line, FONT, TEXT, w);
        }

        let lines = self.canvas.wrap(&state.status, FONT, w - 80);
        for (i, line) in lines.iter().enumerate() {
            let lw = self.canvas.text_width(line, FONT);
            self.canvas.text((w - lw) / 2, h - 50 + i as i32 * LINE_H, line, FONT, MUTED, w);
        }
    }

    /// What Return and the "Log in" / "Verify" words do.
    fn submit_login(&mut self) {
        let stage = self.shared.lock().map(|s| s.login_stage).unwrap_or(LoginStage::Form);
        match stage {
            LoginStage::Form => {
                if self.login_fields[0].trim().is_empty() || self.login_fields[1].is_empty() {
                    return;
                }
                let _ = self.actions.send(Action::PasswordLogin {
                    login: self.login_fields[0].clone(),
                    password: std::mem::take(&mut self.login_fields[1]),
                });
            }
            LoginStage::Code => {
                let _ = self.actions.send(Action::LoginCode { code: std::mem::take(&mut self.login_fields[2]) });
            }
            LoginStage::Busy => {}
        }
        self.dirty = true;
    }

    fn click_login(&mut self, x: i32, y: i32) {
        if let Some((index, ..)) = self.login_field_rects.iter().find(|(_, fx, fy, fw, fh)| x >= *fx && x < fx + fw && y >= *fy && y < fy + fh) {
            self.login_focus = *index;
            self.dirty = true;
            return;
        }
        let hit = self.login_buttons.iter().find(|(_, x0, x1, by)| x >= *x0 - 4 && x <= *x1 + 4 && y >= by - 18 && y <= by + 8).map(|b| b.0);
        match hit {
            Some("Log in") | Some("Verify") => self.submit_login(),
            Some("Cancel") => {
                self.login_fields[2].clear();
                let _ = self.actions.send(Action::LoginCancel);
            }
            Some("Text me a code") => {
                let _ = self.actions.send(Action::LoginSendSms);
            }
            Some("Use a backup code") => {
                let _ = self.actions.send(Action::LoginUseKind(CodeKind::Backup));
            }
            Some("Use a 6-digit code") => {
                let _ = self.actions.send(Action::LoginUseKind(CodeKind::Totp));
            }
            _ => {}
        }
        self.dirty = true;
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
                    let joined = channel.id == state.voice_channel_id;
                    (format!("(voice) {}", channel.name), if joined { ACCENT } else { TEXT })
                } else if guild.id == "@me" {
                    (channel.name.clone(), if selected { WHITE } else { TEXT })
                } else {
                    (format!("# {}", channel.name), if selected { WHITE } else { TEXT })
                };
                self.canvas.text(GUILDS_W + 12, y + 17, &label, FONT, color, GUILDS_W + CHANNELS_W - 8);
                if !channel.is_category {
                    self.channel_rows.push((y, channel.id.clone(), channel.is_voice, channel.name.clone()));
                }
            }
        }

        // Voice bar at the bottom of the channel column while in a voice channel.
        self.voice_bar = None;
        if !state.voice_channel_id.is_empty() {
            let people = state.voice_users.len().min(6) as i32;
            let bar_h = 58 + people * LINE_H;
            let top = h - bar_h;
            let right = GUILDS_W + CHANNELS_W - 8;
            self.canvas.rect(GUILDS_W, top, CHANNELS_W, bar_h, SIDEBAR);
            let state_text = if state.voice_connected { "Voice connected" } else { "Connecting voice..." };
            self.canvas.text(GUILDS_W + 12, top + 18, state_text, FONT, if state.voice_connected { ACCENT } else { MUTED }, right);
            self.canvas.text(GUILDS_W + 12, top + 36, &state.voice_channel_name, FONT, TEXT, right);
            for (i, name) in state.voice_users.iter().take(6).enumerate() {
                self.canvas.text(GUILDS_W + 20, top + 54 + i as i32 * LINE_H, name, FONT, MUTED, right);
            }
            let y = h - 8;
            let mute_label = if state.muted { "Unmute" } else { "Mute" };
            let mute_from = GUILDS_W + 12;
            let mute_to = self.canvas.text(mute_from, y, mute_label, FONT, if state.muted { ACCENT } else { TEXT }, right);
            let leave_from = mute_to + 24;
            let leave_to = self.canvas.text(leave_from, y, "Leave", FONT, TEXT, right);
            self.voice_bar = Some((top, (mute_from, mute_to), (leave_from, leave_to)));
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
        if !self.logged_in {
            self.click_login(x, y);
            return;
        }
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
            if let Some((top, mute, leave)) = self.voice_bar {
                if y >= top {
                    // The two words on the last line of the bar are its buttons.
                    if y >= self.canvas.h - 28 {
                        if x >= mute.0 && x <= mute.1 {
                            let _ = self.actions.send(Action::ToggleMute);
                        } else if x >= leave.0 && x <= leave.1 {
                            let _ = self.actions.send(Action::LeaveVoice);
                        }
                    }
                    return;
                }
            }
            let hit = self
                .channel_rows
                .iter()
                .find(|(ry, ..)| y >= *ry && y < *ry + ROW_H)
                .map(|(_, id, is_voice, name)| (id.clone(), *is_voice, name.clone()));
            if let Some((channel_id, is_voice, name)) = hit {
                let guild_id = self
                    .shared
                    .lock()
                    .ok()
                    .and_then(|s| s.guilds.get(self.guild).map(|g| g.id.clone()))
                    .unwrap_or_default();
                if is_voice {
                    let _ = self.actions.send(Action::JoinVoice { guild_id, channel_id, name });
                } else {
                    self.message_scroll = 0;
                    let _ = self.actions.send(Action::SelectChannel { guild_id, channel_id });
                }
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
        if self.logged_in {
            self.input.push(character);
        } else {
            self.login_fields[self.login_focus].push(character);
        }
        self.dirty = true;
    }

    fn key_down_event(&mut self, keycode: KeyCode, _keymods: KeyMods, _repeat: bool) {
        if !self.logged_in {
            match keycode {
                KeyCode::Backspace => {
                    self.login_fields[self.login_focus].pop();
                }
                KeyCode::Tab => {
                    if self.login_focus < 2 {
                        self.login_focus = 1 - self.login_focus;
                    }
                }
                KeyCode::Enter | KeyCode::KpEnter => {
                    // Return in the email field moves on to the password.
                    if self.login_focus == 0 && self.login_fields[1].is_empty() {
                        self.login_focus = 1;
                    } else {
                        self.submit_login();
                    }
                }
                _ => {}
            }
            self.dirty = true;
            return;
        }
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

/// `--self-test`: checks without a window or a login that the voice building blocks work on
/// this machine: the Opus codec (encode a tone, decode it, compare loudness) and a DAVE
/// encryption session (key generation through OpenMLS). Prints PASS/FAIL lines.
fn self_test() -> bool {
    use opus_rs::{Application, OpusDecoder, OpusEncoder};
    let mut ok = true;

    let opus = (|| -> Result<String, String> {
        let mut encoder = OpusEncoder::new(48000, 2, Application::Voip).map_err(|e| format!("encoder: {e:?}"))?;
        let mut decoder = OpusDecoder::new(48000, 2).map_err(|e| format!("decoder: {e:?}"))?;
        let (mut in_energy, mut out_energy, mut bytes) = (0.0f64, 0.0f64, 0usize);
        let mut phase = 0.0f32;
        for frame in 0..25 {
            let mut pcm = [0.0f32; 1920];
            for pair in pcm.chunks_mut(2) {
                phase = (phase + 440.0 / 48000.0) % 1.0;
                let v = (phase * std::f32::consts::TAU).sin() * 0.3;
                pair[0] = v;
                pair[1] = v;
            }
            let mut packet = vec![0u8; 1000];
            let len = encoder.encode(&pcm, 960, &mut packet).map_err(|e| format!("encode: {e:?}"))?;
            let mut out = vec![0.0f32; 11520];
            let samples = decoder.decode(&packet[..len], 5760, &mut out).map_err(|e| format!("decode: {e:?}"))?;
            // The first frames carry the codec's start-up delay.
            if frame >= 5 {
                bytes += len;
                in_energy += pcm.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
                out_energy += out[..samples * 2].iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
            }
        }
        let ratio = out_energy / in_energy.max(1e-9);
        if !(0.5..2.0).contains(&ratio) {
            return Err(format!("decoded loudness is {ratio:.2} times the original"));
        }
        Ok(format!("20 frames, {} bytes each on average, decoded loudness {ratio:.2} of the original", bytes / 20))
    })();
    match opus {
        Ok(detail) => println!("PASS  opus codec: {detail}"),
        Err(detail) => {
            println!("FAIL  opus codec: {detail}");
            ok = false;
        }
    }

    match davey::DaveSession::new(std::num::NonZeroU16::new(1).unwrap(), 123456789012345678, 987654321098765432, None) {
        Ok(_) => println!("PASS  dave session: created (keys generated through OpenMLS)"),
        Err(e) => {
            println!("FAIL  dave session: {e:?}");
            ok = false;
        }
    }

    println!("{}", if ok { "RESULT: all checks passed" } else { "RESULT: at least one check failed" });
    ok
}

fn main() {
    litecord_legacy_compat::init();
    if std::env::args().any(|a| a == "--self-test") {
        std::process::exit(if self_test() { 0 } else { 1 });
    }
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
