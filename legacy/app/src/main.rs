//! Litecord legacy edition: a small window for old Intel Macs (OS X 10.5 - 10.11) on top of
//! Litecord's own network and voice code: text chat and voice channels. The interface is drawn on the CPU
//! into one picture and shown through OpenGL 2.1 (miniquad with the old-Mac changes of
//! TheLastRoute), the path proven on a Mac Pro 3,1 with OS X 10.6.8.

mod accounts;
mod backend;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use backend::{spans_from_text, Action, CodeKind, LoginStage, Picture, Shared, SharedRef, Span};

/// Pictures by address, as the backend keeps them.
type Images = HashMap<String, Option<Arc<Picture>>>;

/// Fonts of the system tried, in this order, for characters the bundled font lacks
/// (symbols, Chinese, Japanese, Korean...). Each is read the first time it is needed.
const FALLBACK_FONTS: [&str; 9] = [
    "/System/Library/Fonts/Apple Symbols.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/System/Library/Fonts/AppleGothic.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
];
use miniquad::*;
use tokio::sync::mpsc::UnboundedSender;

const BG: [u8; 3] = [0x31, 0x33, 0x38];
const PANEL: [u8; 3] = [0x2B, 0x2D, 0x31];
const INPUT: [u8; 3] = [0x38, 0x3A, 0x40];
const ACCENT: [u8; 3] = [0x58, 0x65, 0xF2];
const TEXT: [u8; 3] = [0xDB, 0xDE, 0xE1];
const MUTED: [u8; 3] = [0x94, 0x9B, 0xA4];
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

/// Layout, after Discord's: a rail of round server icons, the channel list with the
/// server's name above and the user below, then the chat with a header and the input.
const GUILDS_W: i32 = 72;
const CHANNELS_W: i32 = 240;
const TOP_H: i32 = 48;
const INPUT_H: i32 = 60;
const ROW_H: i32 = 32;
const LINE_H: i32 = 21;
const ICON: i32 = 48;
const AVATAR: i32 = 40;
const USER_H: i32 = 52;
const RAIL: [u8; 3] = [0x1E, 0x1F, 0x22];
const USER_PANEL: [u8; 3] = [0x23, 0x24, 0x28];
const HOVER: [u8; 3] = [0x2E, 0x30, 0x35];
const LINK: [u8; 3] = [0x00, 0xA8, 0xFC];
const DANGER: [u8; 3] = [0xED, 0x42, 0x45];
const SMALL: f32 = 12.0;
const FONT: f32 = 15.0;
/// Height of the line between the messages and the input.
const INFO_H: i32 = 22;

/// Reactions offered for a selected message: the action's name, and the emoji it gives.
const QUICK_REACTIONS: [(&str, &str); 4] = [("+1", "\u{1F44D}"), ("<3", "\u{2764}\u{FE0F}"), ("haha", "\u{1F602}"), ("party", "\u{1F389}")];

/// A row of the account switcher above the user panel.
#[derive(Clone)]
enum Switcher {
    Switch(String),
    Add,
    LogOut,
}

/// What Return does with the text in the input.
enum Compose {
    Normal,
    Reply { id: String, author: String },
    Edit { id: String },
}

/// The picture the interface is drawn into, with the text rasterizer.
struct Canvas {
    w: i32,
    h: i32,
    px: Vec<u8>,
    /// The bundled font first, then the system fonts read so far.
    fonts: Vec<fontdue::Font>,
    /// System fonts not tried yet.
    fallbacks: Vec<&'static str>,
    glyphs: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

impl Canvas {
    fn new(w: i32, h: i32) -> Canvas {
        let font = fontdue::Font::from_bytes(
            include_bytes!("../../../assets/fonts/Inter-Variable.ttf") as &[u8],
            fontdue::FontSettings::default(),
        )
        .expect("the bundled font is readable");
        let mut fallbacks: Vec<&'static str> = FALLBACK_FONTS.to_vec();
        fallbacks.reverse(); // popped from the end, so the first of the list is tried first
        Canvas { w, h, px: vec![0; (w * h * 4) as usize], fonts: vec![font], fallbacks, glyphs: HashMap::new() }
    }

    /// The font to draw `ch` with: the first that has it, reading more system fonts if needed.
    fn font_for(&mut self, ch: char) -> usize {
        if let Some(i) = self.fonts.iter().position(|f| f.lookup_glyph_index(ch) != 0) {
            return i;
        }
        // Nothing to look for: invisible characters, and the joiners and selectors of emoji.
        if ch.is_whitespace() || ch.is_control() || matches!(ch as u32, 0x200B..=0x200F | 0xFE00..=0xFE0F | 0x2060..=0x206F | 0xE0000..=0xE007F) {
            return 0;
        }
        while let Some(path) = self.fallbacks.pop() {
            let Ok(bytes) = std::fs::read(path) else { continue };
            let started = std::time::Instant::now();
            match fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                Ok(font) => {
                    log::info!("extra font {path} read in {} ms", started.elapsed().as_millis());
                    let has = font.lookup_glyph_index(ch) != 0;
                    self.fonts.push(font);
                    if has {
                        return self.fonts.len() - 1;
                    }
                }
                Err(e) => log::warn!("extra font {path} not usable: {e}"),
            }
        }
        0
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
        if !self.glyphs.contains_key(&key) {
            let font = self.font_for(ch);
            let glyph = self.fonts[font].rasterize(ch, size);
            self.glyphs.insert(key, glyph);
        }
        &self.glyphs[&key]
    }

    fn blend(&mut self, x: i32, y: i32, c: [u8; 3], alpha: u32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || alpha == 0 {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        for k in 0..3 {
            let old = self.px[i + k] as u32;
            self.px[i + k] = ((c[k] as u32 * alpha + old * (255 - alpha)) / 255) as u8;
        }
        self.px[i + 3] = 0xFF;
    }

    /// How much of the pixel at (dx, dy) of a w x h box lies inside its rounded shape, 0..255.
    fn corner_alpha(dx: i32, dy: i32, w: i32, h: i32, r: i32) -> u32 {
        let cx = if dx < r { r } else if dx >= w - r { w - r - 1 } else { dx };
        let cy = if dy < r { r } else if dy >= h - r { h - r - 1 } else { dy };
        if cx == dx || cy == dy {
            return 255;
        }
        let d = (((dx - cx) * (dx - cx) + (dy - cy) * (dy - cy)) as f32).sqrt();
        ((r as f32 + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u32
    }

    fn round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: [u8; 3]) {
        let r = r.min(w / 2).min(h / 2).max(0);
        for dy in 0..h {
            for dx in 0..w {
                self.blend(x + dx, y + dy, c, Self::corner_alpha(dx, dy, w, h, r));
            }
        }
    }

    /// Draws a picture scaled to `size` x `size`; `radius` rounds its corners (size / 2: a circle).
    fn image(&mut self, picture: &Picture, x: i32, y: i32, size: i32, radius: i32) {
        let (pw, ph) = (picture.w as i32, picture.h as i32);
        if pw == 0 || ph == 0 || size <= 0 {
            return;
        }
        for dy in 0..size {
            for dx in 0..size {
                let shape = if radius > 0 { Self::corner_alpha(dx, dy, size, size, radius) } else { 255 };
                if shape == 0 {
                    continue;
                }
                // Average of the source pixels this pixel covers (the pictures are larger than drawn).
                let x0 = dx * pw / size;
                let x1 = (((dx + 1) * pw + size - 1) / size).min(pw).max(x0 + 1);
                let y0 = dy * ph / size;
                let y1 = (((dy + 1) * ph + size - 1) / size).min(ph).max(y0 + 1);
                let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let i = ((sy * pw + sx) * 4) as usize;
                        let pa = picture.rgba[i + 3] as u32;
                        r += picture.rgba[i] as u32 * pa;
                        g += picture.rgba[i + 1] as u32 * pa;
                        b += picture.rgba[i + 2] as u32 * pa;
                        a += pa;
                        n += 1;
                    }
                }
                if a == 0 {
                    continue;
                }
                let colour = [(r / a) as u8, (g / a) as u8, (b / a) as u8];
                self.blend(x + dx, y + dy, colour, (a / n) * shape / 255);
            }
        }
    }

    /// Text drawn twice, one pixel apart: the bundled font file has one weight only.
    fn bold(&mut self, x: i32, y: i32, text: &str, size: f32, c: [u8; 3], max_x: i32) -> i32 {
        self.text(x, y, text, size, c, max_x);
        self.text(x + 1, y, text, size, c, max_x) + 1
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
    /// The window's OpenGL side. None when drawing to a picture file (`--render-demo`).
    gl: Option<(Box<dyn RenderingBackend>, Pipeline, Bindings)>,
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
    /// Message picked with a click, the reply/edit state, and what was drawn last frame for
    /// clicks: the top of each message line with its message id, and the action words.
    selected: Option<String>,
    compose: Compose,
    message_rows: Vec<(i32, i32, String)>,
    /// Pictures asked for already, and the ones the last frame missed.
    requested: std::collections::HashSet<String>,
    wanted: Vec<String>,
    /// "Log out" in the user panel: x from, x to, y.
    logout: (i32, i32, i32),
    /// The account switcher: open, its rows (top, bottom, what they do) for clicks.
    switcher_open: bool,
    switcher_rows: Vec<(i32, i32, Switcher)>,
    /// "Attach file" in the line above the input: x from, x to, y from, y to.
    attach_rect: Option<(i32, i32, i32, i32)>,
    /// Saved accounts on the login screen: (top, bottom, account key).
    login_account_rows: Vec<(i32, i32, String)>,
    action_buttons: Vec<(&'static str, i32, i32, i32)>,
    last_typing_sent: Option<std::time::Instant>,
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
        Stage::with(canvas, Some((ctx, pipeline, bindings)), shared, actions)
    }

    fn with(canvas: Canvas, gl: Option<(Box<dyn RenderingBackend>, Pipeline, Bindings)>, shared: SharedRef, actions: UnboundedSender<Action>) -> Stage {
        Stage {
            gl,
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
            selected: None,
            compose: Compose::Normal,
            message_rows: Vec::new(),
            requested: std::collections::HashSet::new(),
            wanted: Vec::new(),
            logout: (i32::MAX, i32::MAX, 0),
            switcher_open: false,
            switcher_rows: Vec::new(),
            attach_rect: None,
            login_account_rows: Vec::new(),
            action_buttons: Vec::new(),
            last_typing_sent: None,
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
        drop(state);
        // Pictures the frame needed and did not have: ask for each once.
        for url in std::mem::take(&mut self.wanted) {
            if self.requested.insert(url.clone()) {
                let _ = self.actions.send(Action::FetchImage(url));
            }
        }
    }

    /// A picture if it is loaded; otherwise notes that it is wanted.
    fn picture(&mut self, images: &Images, url: &str) -> Option<Arc<Picture>> {
        if url.is_empty() {
            return None;
        }
        match images.get(url) {
            Some(picture) => picture.clone(),
            None => {
                self.wanted.push(url.to_string());
                None
            }
        }
    }

    /// A round picture, or a coloured shape with initials while there is none.
    fn avatar(&mut self, images: &Images, url: &str, name: &str, x: i32, y: i32, size: i32, radius: i32) {
        if let Some(picture) = self.picture(images, url) {
            self.canvas.image(&picture, x, y, size, radius);
            return;
        }
        // A colour picked by the name, from Discord's default avatar colours.
        let colours = [[0x58, 0x65, 0xF2], [0x75, 0x7E, 0x8A], [0x3B, 0xA5, 0x5C], [0xFA, 0xA6, 0x1A], [0xED, 0x42, 0x45], [0xEB, 0x45, 0x9E]];
        let pick = name.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize)) % colours.len();
        self.canvas.round_rect(x, y, size, size, radius, colours[pick]);
        let initials: String = name.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphanumeric())).take(2).collect::<String>().to_uppercase();
        let text_size = size as f32 * 0.4;
        let tw = self.canvas.text_width(&initials, text_size);
        self.canvas.text(x + (size - tw) / 2, y + size / 2 + (text_size * 0.36) as i32, &initials, text_size, WHITE, x + size);
    }

    fn span_width(&mut self, span: &Span, size: f32) -> i32 {
        match span {
            Span::Text(text) | Span::Link(text) => self.canvas.text_width(text, size),
            Span::Emoji(..) => (size * 1.35) as i32 + 2,
        }
    }

    /// Draws spans on one line with the baseline at `y`; returns the x after them.
    fn spans(&mut self, images: &Images, x: i32, y: i32, spans: &[Span], size: f32, colour: [u8; 3], max_x: i32) -> i32 {
        let mut pen = x;
        for span in spans {
            match span {
                Span::Text(text) => pen = self.canvas.text(pen, y, text, size, colour, max_x),
                Span::Link(text) => pen = self.canvas.text(pen, y, text, size, LINK, max_x),
                Span::Emoji(url, alt) => {
                    let side = (size * 1.35) as i32;
                    if pen + side > max_x {
                        break;
                    }
                    match self.picture(images, url) {
                        Some(picture) => self.canvas.image(&picture, pen, y - (size * 1.05) as i32, side, 0),
                        // Not loaded (yet): a custom emoji shows its :name:, a Unicode one stays blank.
                        None if alt.is_ascii() => {
                            self.canvas.text(pen, y, alt, SMALL, MUTED, pen + side);
                        }
                        None => {}
                    }
                    pen += side + 2;
                }
            }
        }
        pen
    }

    /// Adds one piece to the last line, starting a new line when it does not fit.
    fn place(&mut self, lines: &mut Vec<Vec<Span>>, used: &mut i32, piece: Span, size: f32, max_w: i32) {
        let width = self.span_width(&piece, size);
        if *used + width > max_w && *used > 0 {
            lines.push(Vec::new());
            *used = 0;
        }
        *used += width;
        let line = lines.last_mut().expect("there is always a line");
        // Words of the same kind next to each other become one span again.
        match (line.last_mut(), &piece) {
            (Some(Span::Text(last)), Span::Text(word)) => last.push_str(word),
            (Some(Span::Link(last)), Span::Link(word)) => last.push_str(word),
            _ => line.push(piece),
        }
    }

    /// Breaks spans into lines no wider than `max_w`, at spaces.
    fn wrap_spans(&mut self, spans: &[Span], size: f32, max_w: i32) -> Vec<Vec<Span>> {
        let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
        let mut used = 0;
        for span in spans {
            match span {
                Span::Emoji(..) => self.place(&mut lines, &mut used, span.clone(), size, max_w),
                Span::Text(text) | Span::Link(text) => {
                    let link = matches!(span, Span::Link(_));
                    for word in text.split_inclusive(' ') {
                        // A word wider than the pane (a long address) is cut letter by letter.
                        if self.canvas.text_width(word, size) > max_w {
                            for ch in word.chars() {
                                let piece = if link { Span::Link(ch.to_string()) } else { Span::Text(ch.to_string()) };
                                self.place(&mut lines, &mut used, piece, size, max_w);
                            }
                        } else {
                            let piece = if link { Span::Link(word.to_string()) } else { Span::Text(word.to_string()) };
                            self.place(&mut lines, &mut used, piece, size, max_w);
                        }
                    }
                }
            }
        }
        lines
    }

    fn paint_login(&mut self, state: &Shared) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let images = &state.images;
        self.guild_rows.clear();
        self.channel_rows.clear();
        self.logout = (i32::MAX, i32::MAX, 0);
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

        // Saved accounts: one click signs in with that one. Back returns from adding one.
        self.login_account_rows.clear();
        if !state.accounts.is_empty() {
            let mut ay = y + 20 + lines.len() as i32 * LINE_H + 20;
            self.canvas.text(fx, ay, "Saved accounts", SMALL, MUTED, w);
            ay += 12;
            for account in &state.accounts {
                self.canvas.round_rect(fx, ay, fw, 30, 6, INPUT);
                self.avatar(images, &account.avatar, &account.name, fx + 6, ay + 5, 20, 10);
                let who = spans_from_text(&account.name);
                self.spans(images, fx + 34, ay + 20, &who, FONT, TEXT, fx + fw - 70);
                self.canvas.text(fx + fw - 62, ay + 20, "Use", SMALL + 1.0, ACCENT, fx + fw);
                self.login_account_rows.push((ay, ay + 30, account.key.clone()));
                ay += 36;
            }
            if state.adding {
                button(self, "Back", fx, ay + 6, false);
            }
        } else if state.adding {
            button(self, "Back", fx, y + 20 + lines.len() as i32 * LINE_H + 24, false);
        }
        let lines = self.canvas.wrap(&state.status, FONT, w - 80);
        for (i, line) in lines.iter().enumerate() {
            let lw = self.canvas.text_width(line, FONT);
            self.canvas.text((w - lw) / 2, h - 50 + i as i32 * LINE_H, line, FONT, MUTED, w);
        }
    }

    /// A click in the message pane: an action word of the selected message, or a message.
    fn click_messages(&mut self, x: i32, y: i32) {
        self.dirty = true;
        let action = self.action_buttons.iter().find(|(_, x0, x1, by)| x >= *x0 && x <= *x1 && y >= by - 18 && y <= by + 8).map(|b| b.0);
        if let (Some(action), Some(id)) = (action, self.selected.clone()) {
            let (channel_id, author, text) = match self.shared.lock() {
                Ok(s) => match s.messages.iter().find(|m| m.id == id) {
                    Some(m) => (s.channel_id.clone(), m.author.clone(), m.text.clone()),
                    None => return,
                },
                Err(_) => return,
            };
            match action {
                "Reply" => self.compose = Compose::Reply { id, author },
                "Edit" => {
                    self.input = text;
                    self.compose = Compose::Edit { id };
                }
                "Delete" => {
                    let _ = self.actions.send(Action::Delete { channel_id, message_id: id });
                }
                "Close" => {}
                label => {
                    if let Some((_, emoji)) = QUICK_REACTIONS.iter().find(|(l, _)| *l == label) {
                        let _ = self.actions.send(Action::React { channel_id, message_id: id, key: emoji.to_string(), label: emoji.to_string() });
                    }
                }
            }
            self.selected = None;
            return;
        }
        let hit = self.message_rows.iter().find(|(top, bottom, _)| y >= *top && y < *bottom).map(|(_, _, id)| id.clone());
        // A second click on the same message, or a click on empty space, takes the selection away.
        self.selected = if hit == self.selected { None } else { hit };
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

    /// Asks for a file to send to the open channel: the system's dialog runs in the backend.
    fn request_pick(&mut self) {
        let channel_id = self.shared.lock().map(|s| s.channel_id.clone()).unwrap_or_default();
        if channel_id.is_empty() {
            return;
        }
        let reply_to = match &self.compose {
            Compose::Reply { id, .. } => Some(id.clone()),
            _ => None,
        };
        self.compose = Compose::Normal;
        let _ = self.actions.send(Action::PickFile { channel_id, reply_to });
        self.dirty = true;
    }

    fn click_login(&mut self, x: i32, y: i32) {
        if let Some((_, _, key)) = self.login_account_rows.iter().find(|(top, bottom, _)| y >= *top && y < *bottom && x >= 0) {
            let _ = self.actions.send(Action::SwitchAccount(key.clone()));
            self.dirty = true;
            return;
        }
        if let Some((index, ..)) = self.login_field_rects.iter().find(|(_, fx, fy, fw, fh)| x >= *fx && x < fx + fw && y >= *fy && y < fy + fh) {
            self.login_focus = *index;
            self.dirty = true;
            return;
        }
        let hit = self.login_buttons.iter().find(|(_, x0, x1, by)| x >= *x0 - 4 && x <= *x1 + 4 && y >= by - 18 && y <= by + 8).map(|b| b.0);
        match hit {
            Some("Back") => {
                let _ = self.actions.send(Action::LoginBack);
            }
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
        let images = &state.images;
        if self.guild >= state.guilds.len() {
            self.guild = 0;
        }
        let mx = GUILDS_W + CHANNELS_W;

        // ---- Chat: messages first; the bars around them are drawn over what sticks out.
        let text_x = mx + 16 + AVATAR + 16;
        let pane_w = (w - text_x - 24).max(60);
        let info_y = h - INPUT_H - 6;
        let top = TOP_H;
        let bottom = h - INPUT_H - INFO_H;

        /// What one message takes on screen.
        struct Block {
            index: usize,
            /// Starts with the author's picture, name and time.
            header: bool,
            lines: Vec<(Vec<Span>, [u8; 3])>,
            height: i32,
        }
        let mut blocks: Vec<Block> = Vec::new();
        for (index, message) in state.messages.iter().enumerate() {
            // Messages of the same author in a row share one author line, as on Discord.
            let header = match index.checked_sub(1).map(|p| &state.messages[p]) {
                Some(previous) => previous.author != message.author || !message.reply_to.is_empty(),
                None => true,
            };
            let mut lines = Vec::new();
            for line in &message.lines {
                for wrapped in self.wrap_spans(line, FONT, pane_w) {
                    lines.push((wrapped, TEXT));
                }
            }
            for line in &message.embed_lines {
                for wrapped in self.wrap_spans(line, FONT, pane_w - 12) {
                    lines.push((wrapped, MUTED));
                }
            }
            let mut height = lines.len() as i32 * LINE_H + if message.reactions.is_empty() { 0 } else { 28 };
            if header {
                height = (height + 22).max(AVATAR + 2) + 16 + if message.reply_to.is_empty() { 0 } else { 18 };
            } else {
                height += 2;
            }
            blocks.push(Block { index, header, lines, height });
        }
        let total: i32 = blocks.iter().map(|b| b.height).sum();
        let max_scroll = (total - (bottom - top) + 8).max(0);
        self.message_scroll = self.message_scroll.clamp(0, max_scroll);
        let selected_index = self.selected.as_ref().and_then(|id| state.messages.iter().position(|m| m.id == *id));
        if self.selected.is_some() && selected_index.is_none() {
            self.selected = None; // the message is gone
        }
        self.message_rows.clear();
        let mut y = bottom - total + self.message_scroll - 6;
        for block in &blocks {
            let message = &state.messages[block.index];
            let (block_top, block_bottom) = (y, y + block.height);
            y = block_bottom;
            if block_bottom < top || block_top > bottom {
                continue;
            }
            let mut line_y = block_top;
            if block.header {
                line_y += 16;
            }
            if Some(block.index) == selected_index {
                self.canvas.rect(mx, line_y - 2, w - mx, block_bottom - line_y + 2, HOVER);
            }
            self.message_rows.push((line_y, block_bottom, message.id.clone()));
            if block.header {
                if !message.reply_to.is_empty() {
                    let reply = spans_from_text(&format!("re: {}", message.reply_to));
                    self.spans(images, text_x, line_y + 12, &reply, SMALL + 1.0, MUTED, w - 16);
                    line_y += 18;
                }
                self.avatar(images, &message.avatar_url, &message.author, mx + 16, line_y + 2, AVATAR, AVATAR / 2);
                let name = spans_from_text(&message.author);
                // Twice, one pixel apart, for a bolder name.
                self.spans(images, text_x + 1, line_y + 16, &name, FONT, WHITE, w - 16);
                let name_end = self.spans(images, text_x, line_y + 16, &name, FONT, WHITE, w - 16);
                self.canvas.text(name_end + 9, line_y + 16, &message.time, SMALL, MUTED, w - 16);
                line_y += 22;
            }
            for (spans, colour) in &block.lines {
                if *colour == MUTED {
                    // An embed: a bar on the left, as on Discord.
                    self.canvas.rect(text_x, line_y + 2, 3, LINE_H - 2, PANEL);
                    self.spans(images, text_x + 12, line_y + 16, spans, FONT, MUTED, w - 16);
                } else {
                    self.spans(images, text_x, line_y + 16, spans, FONT, *colour, w - 16);
                }
                line_y += LINE_H;
            }
            // Reaction chips: the emoji picture and the count; yours has an accent border.
            let mut chip_x = text_x;
            for reaction in &message.reactions {
                let spans = spans_from_text(&format!("{} {}", reaction.label, reaction.count));
                let mut chip_w = 16;
                for span in &spans {
                    chip_w += self.span_width(span, SMALL + 1.0);
                }
                if reaction.me {
                    self.canvas.round_rect(chip_x - 1, line_y + 3, chip_w + 2, 24, 9, ACCENT);
                }
                self.canvas.round_rect(chip_x, line_y + 4, chip_w, 22, 8, if reaction.me { [0x3A, 0x3F, 0x66] } else { PANEL });
                self.spans(images, chip_x + 8, line_y + 20, &spans, SMALL + 1.0, TEXT, w - 16);
                chip_x += chip_w + 6;
            }
        }
        if state.channel_id.is_empty() {
            self.canvas.text(mx + 24, top + 48, "Choose a server on the left, then a channel.", FONT, MUTED, w - 8);
        }

        // ---- Chat header: the channel's name, and what the app has to say.
        self.canvas.rect(mx, 0, w - mx, TOP_H, BG);
        self.canvas.rect(mx, TOP_H - 1, w - mx, 1, RAIL);
        let open_channel = if state.channel_id.is_empty() {
            None
        } else {
            state.guilds.iter().flat_map(|g| g.channels.iter().map(move |c| (g, c))).find(|(_, c)| c.id == state.channel_id)
        };
        let mut header_x = mx + 20;
        if let Some((guild, channel)) = open_channel {
            if guild.id != "@me" {
                header_x = self.canvas.text(header_x, 31, "#", 22.0, MUTED, w) + 8;
            }
            let name = spans_from_text(&channel.name);
            self.spans(images, header_x + 1, 30, &name, 16.0, WHITE, w - 16);
            header_x = self.spans(images, header_x, 30, &name, 16.0, WHITE, w - 16) + 16;
        }
        self.canvas.text(header_x, 30, &state.status, SMALL + 1.0, MUTED, w - 12);

        // ---- The line above the input, and the input.
        self.canvas.rect(mx, bottom, w - mx, h - bottom, BG);
        self.action_buttons.clear();
        if let Some(index) = selected_index {
            let mine = state.messages[index].mine;
            let mut x = mx + 20;
            let mut labels: Vec<&'static str> = vec!["Reply"];
            labels.extend(QUICK_REACTIONS.iter().map(|(label, _)| *label));
            if mine {
                labels.extend(["Edit", "Delete"]);
            }
            labels.push("Close");
            for label in labels {
                // The reaction buttons show their emoji picture.
                let spans = match QUICK_REACTIONS.iter().find(|(l, _)| *l == label) {
                    Some((_, emoji)) => spans_from_text(emoji),
                    None => vec![Span::Text(label.to_string())],
                };
                let mut width = 16;
                for span in &spans {
                    width += self.span_width(span, SMALL + 1.0);
                }
                self.canvas.round_rect(x, info_y - 16, width, 22, 6, PANEL);
                self.spans(images, x + 8, info_y, &spans, SMALL + 1.0, if label == "Delete" { DANGER } else { TEXT }, w - 8);
                self.action_buttons.push((label, x, x + width, info_y));
                x += width + 8;
            }
        } else {
            let notice = match &self.compose {
                Compose::Reply { author, .. } => format!("Replying to {author}   (Esc cancels)"),
                Compose::Edit { .. } => "Editing your message   (Esc cancels)".to_string(),
                Compose::Normal => {
                    let names: Vec<&str> = state.typing.iter().map(|(name, _)| name.as_str()).collect();
                    match names.len() {
                        0 => String::new(),
                        1 => format!("{} is typing...", names[0]),
                        2 | 3 => format!("{} are typing...", names.join(", ")),
                        _ => "Several people are typing...".to_string(),
                    }
                }
            };
            let notice = spans_from_text(&notice);
            self.spans(images, mx + 20, info_y, &notice, SMALL + 1.0, MUTED, w - 8);
        }
        // "Attach file": opens the system's file dialog (Ctrl or Cmd+O does the same).
        self.attach_rect = None;
        if !state.channel_id.is_empty() {
            let label = "Attach file";
            let x1 = w - 24;
            let x0 = x1 - self.canvas.text_width(label, SMALL + 1.0) - 16;
            self.canvas.round_rect(x0, info_y - 16, x1 - x0, 22, 6, PANEL);
            self.canvas.text(x0 + 8, info_y, label, SMALL + 1.0, ACCENT, x1);
            self.attach_rect = Some((x0, x1, info_y - 18, info_y + 8));
        }
        self.canvas.round_rect(mx + 16, h - INPUT_H + 4, w - mx - 32, 44, 8, INPUT);
        let channel_label = open_channel.map(|(g, c)| if g.id == "@me" { format!("@{}", c.name) } else { format!("#{}", c.name) });
        let (mut shown, colour) = if self.input.is_empty() {
            match channel_label {
                Some(label) => (format!("Message {label}      (/upload /path/to/file sends a file; click a message for more)"), MUTED),
                None => (String::new(), MUTED),
            }
        } else {
            (format!("{}|", self.input), TEXT)
        };
        // Keep the end of a long line in view.
        if colour == TEXT {
            while self.canvas.text_width(&shown, FONT) > w - mx - 64 && shown.chars().count() > 1 {
                shown.remove(0);
            }
        }
        let shown = spans_from_text(&shown);
        self.spans(images, mx + 32, h - INPUT_H + 31, &shown, FONT, colour, w - 32);

        // ---- Channel column: channels, then the server name over them, voice bar, user panel.
        self.canvas.rect(GUILDS_W, 0, CHANNELS_W, h, PANEL);
        let right = mx - 8;
        self.channel_rows.clear();
        if let Some(guild) = state.guilds.get(self.guild) {
            let mut channels: Vec<_> = guild.channels.iter().collect();
            if guild.id != "@me" {
                channels.sort_by_key(|c| c.position);
            }
            for (i, channel) in channels.iter().enumerate() {
                let y = TOP_H + 10 + i as i32 * ROW_H - self.channel_scroll;
                if y < TOP_H - ROW_H || y > h {
                    continue;
                }
                if channel.is_category {
                    let upper = spans_from_text(&channel.name.to_uppercase());
                    self.spans(images, GUILDS_W + 16, y + 21, &upper, SMALL, MUTED, right);
                    continue;
                }
                let selected = channel.id == state.channel_id;
                if selected {
                    self.canvas.round_rect(GUILDS_W + 8, y, CHANNELS_W - 16, ROW_H - 2, 4, [0x40, 0x42, 0x49]);
                }
                let joined = channel.id == state.voice_channel_id;
                let colour = if selected || joined { WHITE } else { MUTED };
                let mut x = GUILDS_W + 16;
                if guild.id == "@me" {
                    self.avatar(images, "", &channel.name, x, y + 3, 24, 12);
                    x += 32;
                } else if channel.is_voice {
                    // A small speaker: a box and a cone.
                    let tint = if joined { ACCENT } else { MUTED };
                    self.canvas.rect(x + 2, y + 12, 4, 7, tint);
                    for k in 0..6 {
                        self.canvas.rect(x + 6 + k, y + 12 - k, 1, 7 + 2 * k, tint);
                    }
                    x += 22;
                } else {
                    x = self.canvas.text(x + 2, y + 21, "#", 18.0, MUTED, right) + 8;
                }
                let name = spans_from_text(&channel.name);
                self.spans(images, x, y + 20, &name, FONT, colour, right);
                self.channel_rows.push((y, channel.id.clone(), channel.is_voice, channel.name.clone()));
            }
            // Server name above the list.
            self.canvas.rect(GUILDS_W, 0, CHANNELS_W, TOP_H, PANEL);
            self.canvas.rect(GUILDS_W, TOP_H - 1, CHANNELS_W, 1, RAIL);
            let name = spans_from_text(&guild.name);
            self.spans(images, GUILDS_W + 17, 30, &name, 16.0, WHITE, right);
            self.spans(images, GUILDS_W + 16, 30, &name, 16.0, WHITE, right);
        }

        // Voice bar above the user panel while in a voice channel.
        self.voice_bar = None;
        let panel_top = h - USER_H;
        if !state.voice_channel_id.is_empty() {
            let people = state.voice_users.len().min(6) as i32;
            let bar_h = 74 + people * LINE_H;
            let bar_top = panel_top - bar_h;
            self.canvas.rect(GUILDS_W, bar_top, CHANNELS_W, bar_h, USER_PANEL);
            self.canvas.rect(GUILDS_W, bar_top, CHANNELS_W, 1, RAIL);
            let state_text = if state.voice_connected { "Voice Connected" } else { "Connecting..." };
            self.canvas.bold(GUILDS_W + 16, bar_top + 20, state_text, SMALL + 1.0, if state.voice_connected { [0x23, 0xA5, 0x5A] } else { MUTED }, right);
            let channel = spans_from_text(&state.voice_channel_name);
            self.spans(images, GUILDS_W + 16, bar_top + 37, &channel, SMALL, MUTED, right);
            for (i, name) in state.voice_users.iter().take(6).enumerate() {
                let who = spans_from_text(name);
                self.spans(images, GUILDS_W + 24, bar_top + 56 + i as i32 * LINE_H, &who, SMALL + 1.0, TEXT, right);
            }
            let y = bar_top + bar_h - 12;
            let mute_label = if state.muted { "Unmute" } else { "Mute" };
            let mute_from = GUILDS_W + 16;
            let mute_w = self.canvas.text_width(mute_label, SMALL + 1.0) + 16;
            self.canvas.round_rect(mute_from, y - 16, mute_w, 22, 6, if state.muted { DANGER } else { INPUT });
            self.canvas.text(mute_from + 8, y, mute_label, SMALL + 1.0, WHITE, right);
            let leave_from = mute_from + mute_w + 8;
            let leave_w = self.canvas.text_width("Leave", SMALL + 1.0) + 16;
            self.canvas.round_rect(leave_from, y - 16, leave_w, 22, 6, INPUT);
            self.canvas.text(leave_from + 8, y, "Leave", SMALL + 1.0, WHITE, right);
            self.voice_bar = Some((bar_top, (mute_from, mute_from + mute_w), (leave_from, leave_from + leave_w)));
        }

        // User panel: own picture and name, and the way out.
        self.canvas.rect(GUILDS_W, panel_top, CHANNELS_W, USER_H, USER_PANEL);
        self.avatar(images, &state.my_avatar, &state.user, GUILDS_W + 10, panel_top + 10, 32, 16);
        let logout_w = self.canvas.text_width("Log out", SMALL);
        let logout_x = mx - logout_w - 12;
        let me = spans_from_text(&state.user);
        self.spans(images, GUILDS_W + 51, panel_top + 31, &me, SMALL + 2.0, WHITE, logout_x - 8);
        self.spans(images, GUILDS_W + 50, panel_top + 31, &me, SMALL + 2.0, WHITE, logout_x - 8);
        self.canvas.text(logout_x, panel_top + 31, "Log out", SMALL, MUTED, mx);
        self.logout = (logout_x, mx, panel_top);

        // The account switcher: a popup above the user panel, opened by a click on it.
        self.switcher_rows.clear();
        if self.switcher_open {
            let rows = state.accounts.len() as i32 + 2;
            let box_h = rows * 34 + 12;
            let box_top = panel_top - box_h - 6;
            self.canvas.round_rect(GUILDS_W + 8, box_top, CHANNELS_W - 16, box_h, 8, [0x18, 0x19, 0x1C]);
            let mut ry = box_top + 6;
            for account in &state.accounts {
                if account.active {
                    self.canvas.round_rect(GUILDS_W + 12, ry, CHANNELS_W - 24, 30, 6, [0x3A, 0x3F, 0x66]);
                }
                self.avatar(images, &account.avatar, &account.name, GUILDS_W + 18, ry + 5, 20, 10);
                let who = spans_from_text(&account.name);
                let colour = if account.active { WHITE } else { TEXT };
                self.spans(images, GUILDS_W + 46, ry + 20, &who, FONT, colour, mx - 40);
                if account.active {
                    self.canvas.text(mx - 66, ry + 20, "current", SMALL, MUTED, mx - 8);
                }
                self.switcher_rows.push((ry, ry + 30, Switcher::Switch(account.key.clone())));
                ry += 34;
            }
            for (label, action) in [("Add account", Switcher::Add), ("Log out", Switcher::LogOut)] {
                self.canvas.round_rect(GUILDS_W + 12, ry, CHANNELS_W - 24, 30, 6, INPUT);
                let colour = if matches!(action, Switcher::LogOut) { DANGER } else { TEXT };
                self.canvas.text(GUILDS_W + 26, ry + 20, label, FONT, colour, mx - 20);
                self.switcher_rows.push((ry, ry + 30, action));
                ry += 34;
            }
        }

        // ---- Server rail: round icons, a white pill beside the open one.
        self.canvas.rect(0, 0, GUILDS_W, h, RAIL);
        self.guild_rows.clear();
        for (i, guild) in state.guilds.iter().enumerate() {
            let y = 12 + i as i32 * (ICON + 8) - self.guild_scroll;
            if y < -ICON || y > h {
                continue;
            }
            let open = i == self.guild;
            // The open server's icon is a rounded square, the others are circles.
            let radius = if open { 16 } else { ICON / 2 };
            if guild.id == "@me" {
                self.canvas.round_rect(12, y, ICON, ICON, radius, if open { ACCENT } else { BG });
                let tw = self.canvas.text_width("DM", 16.0);
                self.canvas.bold(12 + (ICON - tw) / 2, y + 30, "DM", 16.0, WHITE, GUILDS_W);
            } else {
                let url = state.guild_icons.get(&guild.id).cloned().unwrap_or_default();
                self.avatar(images, &url, &guild.name, 12, y, ICON, radius);
            }
            if open {
                self.canvas.round_rect(-4, y + 4, 8, ICON - 8, 4, WHITE);
            }
            self.guild_rows.push((y, i));
        }
    }

    fn click(&mut self, x: i32, y: i32) {
        if !self.logged_in {
            self.click_login(x, y);
            return;
        }
        // An open switcher takes this click: its row acts, any other place closes it.
        if self.switcher_open {
            self.switcher_open = false;
            self.dirty = true;
            let row = self.switcher_rows.iter().find(|(top, bottom, _)| y >= *top && y < *bottom).map(|(_, _, what)| what.clone());
            match row {
                Some(Switcher::Switch(key)) => {
                    let _ = self.actions.send(Action::SwitchAccount(key));
                }
                Some(Switcher::Add) => {
                    let _ = self.actions.send(Action::AddAccount);
                }
                Some(Switcher::LogOut) => {
                    let _ = self.actions.send(Action::LogOut);
                }
                None => {}
            }
            return;
        }
        if let Some((x0, x1, y0, y1)) = self.attach_rect {
            if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
                self.request_pick();
                return;
            }
        }
        if x < GUILDS_W {
            if let Some((_, i)) = self.guild_rows.iter().find(|(ry, _)| y >= *ry && y < *ry + ICON) {
                self.guild = *i;
                self.channel_scroll = 0;
                self.dirty = true;
            }
        } else if x < GUILDS_W + CHANNELS_W {
            if y >= self.logout.2 {
                // "Log out" signs this account out; a click elsewhere on the panel opens the switcher.
                if x >= self.logout.0 - 6 && x <= self.logout.1 {
                    let _ = self.actions.send(Action::LogOut);
                } else {
                    self.switcher_open = true;
                }
                self.dirty = true;
                return;
            }
            if let Some((top, mute, leave)) = self.voice_bar {
                if y >= top {
                    // The two buttons on the last line of the bar.
                    if y >= self.logout.2 - 34 {
                        if x >= mute.0 && x <= mute.1 {
                            let _ = self.actions.send(Action::ToggleMute);
                        } else if x >= leave.0 && x <= leave.1 {
                            let _ = self.actions.send(Action::LeaveVoice);
                        }
                    }
                    return;
                }
            }
            if y < TOP_H {
                return;
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
                    self.selected = None;
                    self.compose = Compose::Normal;
                    let _ = self.actions.send(Action::SelectChannel { guild_id, channel_id });
                }
            }
        } else {
            self.click_messages(x, y);
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
        let repaint = self.dirty;
        if repaint {
            self.dirty = false;
            self.paint();
        }
        let Some((ctx, pipeline, bindings)) = self.gl.as_mut() else { return };
        if repaint {
            ctx.texture_update(bindings.images[0], &self.canvas.px);
        }
        ctx.begin_default_pass(PassAction::clear_color(0.0, 0.0, 0.0, 1.0));
        ctx.apply_pipeline(pipeline);
        ctx.apply_bindings(bindings);
        ctx.draw(0, 6, 1);
        ctx.end_render_pass();
        ctx.commit_frame();
    }

    fn resize_event(&mut self, width: f32, height: f32) {
        self.canvas.resize(width as i32, height as i32);
        if let Some((ctx, _, bindings)) = self.gl.as_mut() {
            ctx.delete_texture(bindings.images[0]);
            bindings.images[0] = ctx.new_texture_from_rgba8(self.canvas.w as u16, self.canvas.h as u16, &self.canvas.px);
        }
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
            self.guild_scroll = (self.guild_scroll + step * (ICON + 8)).max(0);
        } else if self.mouse.0 < GUILDS_W + CHANNELS_W {
            self.channel_scroll = (self.channel_scroll + step * ROW_H * 2).max(0);
        } else {
            // Wheel up shows older messages (the scroll is in pixels from the newest).
            self.message_scroll = (self.message_scroll - step * 60).max(0);
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
            // Tell the others, at most every eight seconds (not while editing or typing a command).
            let due = self.last_typing_sent.map_or(true, |at| at.elapsed().as_secs() >= 8);
            if due && !self.input.starts_with('/') && !matches!(self.compose, Compose::Edit { .. }) {
                let channel_id = self.shared.lock().map(|s| s.channel_id.clone()).unwrap_or_default();
                if !channel_id.is_empty() {
                    self.last_typing_sent = Some(std::time::Instant::now());
                    let _ = self.actions.send(Action::Typing { channel_id });
                }
            }
        } else {
            self.login_fields[self.login_focus].push(character);
        }
        self.dirty = true;
    }

    fn key_down_event(&mut self, keycode: KeyCode, keymods: KeyMods, _repeat: bool) {
        if self.logged_in && (keymods.ctrl || keymods.logo) && matches!(keycode, KeyCode::O) {
            self.request_pick();
            return;
        }
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
            KeyCode::Escape => {
                self.switcher_open = false;
                if matches!(self.compose, Compose::Edit { .. }) {
                    self.input.clear();
                }
                self.compose = Compose::Normal;
                self.selected = None;
                self.dirty = true;
            }
            KeyCode::Enter | KeyCode::KpEnter => {
                let text = self.input.trim().to_string();
                let channel_id = self.shared.lock().map(|s| s.channel_id.clone()).unwrap_or_default();
                if !text.is_empty() && !channel_id.is_empty() {
                    let reply_to = match &self.compose {
                        Compose::Reply { id, .. } => Some(id.clone()),
                        _ => None,
                    };
                    if let Some(path) = text.strip_prefix("/upload ") {
                        let _ = self.actions.send(Action::Upload { channel_id, path: path.trim().to_string(), reply_to });
                    } else if let Compose::Edit { id } = &self.compose {
                        let _ = self.actions.send(Action::Edit { channel_id, message_id: id.clone(), text });
                    } else {
                        let _ = self.actions.send(Action::Send { channel_id, text, reply_to });
                    }
                    self.compose = Compose::Normal;
                    self.last_typing_sent = None;
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

/// `--render-demo FILE.png`: draws the interface with made-up servers and messages into a
/// picture file, without a window, a login or the network. For looking at the layout (the
/// CI check keeps the picture) on machines where the app itself cannot be shown.
fn render_demo(path: &str) -> Result<(), String> {
    let shared: SharedRef = Arc::new(Mutex::new(backend::demo_state()));
    let (action_tx, _action_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stage = Stage::with(Canvas::new(1100, 700), None, Arc::clone(&shared), action_tx);
    stage.guild = 1;
    // One message picked, to show its action buttons.
    stage.selected = Some("3".to_string());
    stage.paint();
    // The emoji pictures the first pass asked for, when the network is there; then again.
    let wanted: Vec<String> = stage.requested.iter().cloned().collect();
    if let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
        runtime.block_on(async {
            let client = litecord_legacy::utils::build_generic_http_client();
            for url in wanted {
                let picture = backend::fetch_picture(&client, &url).await;
                println!("{} {url}", if picture.is_some() { "loaded" } else { "MISSING" });
                if let Ok(mut state) = shared.lock() {
                    state.images.insert(url, picture);
                }
            }
        });
    }
    stage.switcher_open = true;
    stage.paint();
    let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), stage.canvas.w as u32, stage.canvas.h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&stage.canvas.px).map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    litecord_legacy_compat::init();
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.iter().any(|a| a == "--self-test") {
        std::process::exit(if self_test() { 0 } else { 1 });
    }
    if let Some(i) = arguments.iter().position(|a| a == "--render-demo") {
        let path = arguments.get(i + 1).cloned().unwrap_or_else(|| "litecord-legacy-demo.png".to_string());
        match render_demo(&path) {
            Ok(()) => println!("wrote {path}"),
            Err(e) => {
                eprintln!("render-demo failed: {e}");
                std::process::exit(1);
            }
        }
        return;
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
            window_width: 1100,
            window_height: 700,
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
