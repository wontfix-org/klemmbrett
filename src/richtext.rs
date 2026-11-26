//! Colored preview text.
//!
//! Terminal color codes are drawn as written, including 24-bit color. When
//! the text has none, `language` selects a syntax: empty is plain, `"auto"`
//! uses the record's file extension, and a name or extension such as `"sql"`
//! forces that syntax.

use crate::theme::Rgba;
use crate::widget::Record;
use std::ops::Range;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub color: Option<(u8, u8, u8)>,
    pub background: Option<(u8, u8, u8)>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rich {
    pub plain: String,
    pub spans: Vec<Span>,
}

/// A dark popup background picks the dark syntax theme.
pub fn dark_background(color: Rgba) -> bool {
    if color.a == 0 {
        return true;
    }
    let luma =
        0.2126 * f32::from(color.r) + 0.7152 * f32::from(color.g) + 0.0722 * f32::from(color.b);
    luma < 160.0
}

/// The path whose extension `"auto"` reads: `value`, else `data.name`, else
/// `data.relative`.
pub fn source_path(record: &Record) -> Option<&str> {
    let value = Some(record.value.as_str()).filter(|path| !path.is_empty());
    let name = record.data.get("name").and_then(|value| value.as_str());
    let relative = record.data.get("relative").and_then(|value| value.as_str());
    [value, name, relative]
        .into_iter()
        .flatten()
        .find(|path| std::path::Path::new(path).extension().is_some())
}

pub fn present(text: &str, language: &str, path: Option<&str>, dark: bool) -> Rich {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if text.contains('\u{1b}') {
        return parse_ansi(&text);
    }
    let Some(language) = resolve_language(language, path) else {
        return plain(text);
    };
    highlight(&text, &language, dark).unwrap_or(plain(text))
}

fn plain(text: String) -> Rich {
    Rich {
        plain: text,
        spans: Vec::new(),
    }
}

fn resolve_language(language: &str, path: Option<&str>) -> Option<String> {
    let language = language.trim();
    if language.is_empty() {
        return None;
    }
    if language.eq_ignore_ascii_case("auto") {
        return path.and_then(|path| {
            std::path::Path::new(path)
                .extension()
                .and_then(|ext| ext.to_str())
                .filter(|ext| !ext.is_empty())
                .map(str::to_owned)
        });
    }
    Some(language.to_owned())
}

/// The syntax for `path`, from its name or extension, else from `first_line`
/// (a shebang or mode line). `None` when nothing matches, including plain text.
pub fn syntax_name(path: &str, first_line: &str) -> Option<String> {
    let (set, _) = engine();
    let path = std::path::Path::new(path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    let syntax = set
        .find_syntax_by_extension(file_name)
        .or_else(|| {
            if extension.is_empty() || extension == file_name {
                None
            } else {
                set.find_syntax_by_extension(extension)
            }
        })
        .or_else(|| set.find_syntax_by_first_line(first_line))?;
    (syntax.name != "Plain Text").then(|| syntax.name.clone())
}

fn engine() -> &'static (SyntaxSet, ThemeSet) {
    static ENGINE: OnceLock<(SyntaxSet, ThemeSet)> = OnceLock::new();
    ENGINE.get_or_init(|| {
        (
            SyntaxSet::load_defaults_newlines(),
            ThemeSet::load_defaults(),
        )
    })
}

fn highlight(text: &str, language: &str, dark: bool) -> Option<Rich> {
    let (set, themes) = engine();
    let syntax = set
        .find_syntax_by_extension(language)
        .or_else(|| set.find_syntax_by_token(language))
        .or_else(|| set.find_syntax_by_name(language))?;
    let theme_name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let theme = themes.themes.get(theme_name)?;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut plain = String::new();
    let mut spans = Vec::new();
    for line in LinesWithEndings::from(text) {
        let regions = highlighter.highlight_line(line, set).ok()?;
        for (style, chunk) in regions {
            let body = chunk.trim_end_matches('\n');
            if !body.is_empty() {
                let start = plain.len();
                let color = style.foreground;
                spans.push(Span {
                    range: start..start + body.len(),
                    color: Some((color.r, color.g, color.b)),
                    background: None,
                    bold: style.font_style.contains(FontStyle::BOLD),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                    underline: style.font_style.contains(FontStyle::UNDERLINE),
                    dim: false,
                });
                plain.push_str(body);
            }
            if chunk.ends_with('\n') {
                plain.push('\n');
            }
        }
    }
    Some(Rich { plain, spans })
}

#[derive(Clone, Copy, Default)]
struct Paint {
    fg: Option<(u8, u8, u8)>,
    bg: Option<(u8, u8, u8)>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    reverse: bool,
}

impl Paint {
    fn span(self, range: Range<usize>) -> Option<Span> {
        let (color, background) = if self.reverse {
            (
                Some(self.bg.unwrap_or((0, 0, 0))),
                Some(self.fg.unwrap_or((229, 229, 229))),
            )
        } else {
            (self.fg, self.bg)
        };
        if color.is_none()
            && background.is_none()
            && !self.bold
            && !self.italic
            && !self.underline
            && !self.dim
        {
            return None;
        }
        Some(Span {
            range,
            color,
            background,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            dim: self.dim,
        })
    }
}

fn parse_ansi(text: &str) -> Rich {
    let bytes = text.as_bytes();
    let mut plain = String::new();
    let mut spans = Vec::new();
    let mut paint = Paint::default();
    let mut span_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            if let Some((end, sgr)) = consume_control(text, i) {
                if let Some(params) = sgr {
                    flush(&plain, &mut span_start, paint, &mut spans);
                    apply_sgr(&mut paint, &params);
                }
                i = end;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
        plain.push(ch);
        i += ch.len_utf8();
    }
    flush(&plain, &mut span_start, paint, &mut spans);
    Rich { plain, spans }
}

fn flush(plain: &str, start: &mut usize, paint: Paint, spans: &mut Vec<Span>) {
    if plain.len() > *start {
        if let Some(span) = paint.span(*start..plain.len()) {
            spans.push(span);
        }
    }
    *start = plain.len();
}

/// A finished escape, and the SGR parameter string when the sequence sets color.
fn consume_control(text: &str, start: usize) -> Option<(usize, Option<String>)> {
    let bytes = text.as_bytes();
    match bytes.get(start + 1)? {
        b'[' => consume_csi(text, start + 2),
        b']' => consume_osc(text, start + 2).map(|end| (end, None)),
        _ => None,
    }
}

fn consume_csi(text: &str, mut i: usize) -> Option<(usize, Option<String>)> {
    let bytes = text.as_bytes();
    let param_start = i;
    while i < bytes.len() {
        let byte = bytes[i];
        if (0x40..=0x7e).contains(&byte) {
            let params = text[param_start..i].to_owned();
            let sgr = (byte == b'm').then_some(params);
            return Some((i + 1, sgr));
        }
        if !(0x20..=0x3f).contains(&byte) {
            return None;
        }
        i += 1;
    }
    Some((bytes.len(), None))
}

fn consume_osc(text: &str, mut i: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i] == 0x07 {
            return Some(i + 1);
        }
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
            return Some(i + 2);
        }
        i += 1;
    }
    Some(bytes.len())
}

fn apply_sgr(paint: &mut Paint, params: &str) {
    if params.is_empty() {
        *paint = Paint::default();
        return;
    }
    let tokens = split_params(params);
    let mut i = 0;
    while i < tokens.len() {
        match parse_u16(tokens[i]) {
            Some(0) => *paint = Paint::default(),
            Some(1) => paint.bold = true,
            Some(2) => paint.dim = true,
            Some(3) => paint.italic = true,
            Some(4 | 21) => paint.underline = true,
            Some(7) => paint.reverse = true,
            Some(22) => {
                paint.bold = false;
                paint.dim = false;
            }
            Some(23) => paint.italic = false,
            Some(24) => paint.underline = false,
            Some(27) => paint.reverse = false,
            Some(39) => paint.fg = None,
            Some(49) => paint.bg = None,
            Some(code) if (30..=37).contains(&code) => {
                paint.fg = Some(ANSI16[(code - 30) as usize]);
            }
            Some(code) if (40..=47).contains(&code) => {
                paint.bg = Some(ANSI16[(code - 40) as usize]);
            }
            Some(code) if (90..=97).contains(&code) => {
                paint.fg = Some(ANSI16[(code - 90 + 8) as usize]);
            }
            Some(code) if (100..=107).contains(&code) => {
                paint.bg = Some(ANSI16[(code - 100 + 8) as usize]);
            }
            Some(38 | 48) => {
                let background = tokens[i] == "48";
                if i + 1 >= tokens.len() {
                    break;
                }
                let (color, last) = match tokens[i + 1] {
                    "5" => read_indexed(&tokens, i + 1),
                    "2" => read_truecolor(&tokens, i + 1),
                    _ => (None, i + 1),
                };
                if let Some(color) = color {
                    if background {
                        paint.bg = Some(color);
                    } else {
                        paint.fg = Some(color);
                    }
                }
                i = last;
            }
            _ => {}
        }
        i += 1;
    }
}

/// `at` is the `5` token. A missing or out-of-range index leaves the color unset.
fn read_indexed(tokens: &[&str], at: usize) -> (Option<(u8, u8, u8)>, usize) {
    let Some(index) = tokens.get(at + 1) else {
        return (None, at);
    };
    (parse_channel(index).map(color_256), at + 1)
}

/// `at` is the `2` token.
///
/// `38;2;r;g;b` has three numbers after the mode. `38:2:0:r:g:b` has four, and
/// the first is a colorspace id. `38:2::r:g:b` has an empty colorspace slot.
/// A missing or out-of-range channel leaves the previous color.
fn read_truecolor(tokens: &[&str], at: usize) -> (Option<(u8, u8, u8)>, usize) {
    let rest = &tokens[at + 1..];
    if rest.is_empty() {
        return (None, at);
    }
    let (channels, consumed) = if rest[0].is_empty() {
        if rest.len() < 4 {
            return (None, tokens.len() - 1);
        }
        (&rest[1..4], 4)
    } else {
        let numeric = rest
            .iter()
            .take_while(|token| parse_u16(token).is_some())
            .count();
        if numeric >= 4 {
            (&rest[1..4], 4)
        } else if numeric >= 3 {
            (&rest[..3], 3)
        } else {
            return (None, at + numeric);
        }
    };
    let color = (|| {
        Some((
            parse_channel(channels[0])?,
            parse_channel(channels[1])?,
            parse_channel(channels[2])?,
        ))
    })();
    (color, at + consumed)
}

fn split_params(params: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (idx, ch) in params.char_indices() {
        if ch == ';' || ch == ':' {
            out.push(&params[start..idx]);
            start = idx + ch.len_utf8();
        }
    }
    out.push(&params[start..]);
    out
}

fn parse_u16(token: &str) -> Option<u16> {
    if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

fn parse_channel(token: &str) -> Option<u8> {
    u8::try_from(parse_u16(token)?).ok()
}

const ANSI16: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

fn color_256(index: u8) -> (u8, u8, u8) {
    if index < 16 {
        return ANSI16[index as usize];
    }
    if index >= 232 {
        let level = 8 + (index - 232) * 10;
        return (level, level, level);
    }
    let index = index - 16;
    let level = |channel: u8| {
        if channel == 0 {
            0
        } else {
            55 + 40 * channel
        }
    };
    (level(index / 36), level((index % 36) / 6), level(index % 6))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color_at(rich: &Rich, index: usize) -> Option<(u8, u8, u8)> {
        rich.spans
            .iter()
            .find(|span| span.range.contains(&index))
            .and_then(|span| span.color)
    }

    #[test]
    fn ansi_red_is_the_16_color_red() {
        let rich = present("\u{1b}[31mred\u{1b}[0m", "sql", None, true);
        assert_eq!(rich.plain, "red");
        assert_eq!(color_at(&rich, 0), Some((205, 0, 0)));
        assert_eq!(rich.spans[0].range, 0..3);
    }

    #[test]
    fn indexed_color_reads_the_cube() {
        let rich = present("\u{1b}[38;5;196mX", "", None, true);
        assert_eq!(color_at(&rich, 0), Some((255, 0, 0)));
    }

    #[test]
    fn truecolor_keeps_the_rgb_triple() {
        for sequence in ["38;2;10;20;30", "38:2::10:20:30", "38:2:0:10:20:30"] {
            let rich = present(&format!("\u{1b}[{sequence}mX"), "", None, true);
            assert_eq!(color_at(&rich, 0), Some((10, 20, 30)), "{sequence}");
        }
    }

    #[test]
    fn a_bad_truecolor_channel_leaves_the_previous_color() {
        let rich = present("\u{1b}[31m\u{1b}[38;2;10;20;300mX", "", None, true);
        assert_eq!(rich.plain, "X");
        assert_eq!(color_at(&rich, 0), Some((205, 0, 0)));
    }

    #[test]
    fn other_control_sequences_are_stripped() {
        let rich = present("\u{1b}[2J\u{1b}]0;title\u{07}hi", "sql", None, true);
        assert_eq!(rich.plain, "hi");
        assert!(rich.spans.is_empty());
    }

    #[test]
    fn ansi_skips_syntax_highlighting() {
        let rich = present("\u{1b}[31mselect\u{1b}[0m", "sql", None, true);
        assert_eq!(color_at(&rich, 0), Some((205, 0, 0)));
    }

    #[test]
    fn sql_select_is_colored_and_an_unknown_language_is_not() {
        let sql = present("select 1", "sql", None, true);
        let keyword = color_at(&sql, 0).expect("select");
        let number = color_at(&sql, sql.plain.find('1').unwrap()).expect("1");
        assert_ne!(keyword, number);
        let plain = present("select 1", "not-a-language", None, true);
        assert!(plain.spans.is_empty());
        let none = present("select 1", "", None, true);
        assert!(none.spans.is_empty());
    }

    #[test]
    fn syntax_name_uses_the_extension_then_the_first_line() {
        assert_eq!(syntax_name("q.sql", "hello").as_deref(), Some("SQL"));
        let python = syntax_name("tool", "#!/usr/bin/env python3").unwrap();
        assert!(python.to_ascii_lowercase().contains("python"), "{python}");
        assert_eq!(syntax_name("notes.txt", "hello"), None);
        let sql = present("select 1", &syntax_name("q.sql", "").unwrap(), None, true);
        assert!(color_at(&sql, 0).is_some());
    }

    #[test]
    fn auto_uses_the_path_extension() {
        let sql = present("select 1", "auto", Some("/home/mbra/sql/q.sql"), true);
        assert!(color_at(&sql, 0).is_some());
        let plain = present("select 1", "auto", None, true);
        assert!(plain.spans.is_empty());
    }
}
