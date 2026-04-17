use std::fmt::{self, Write};

/// Fluent builder for [NomadNet Micron markup](https://markqvist.github.io/Reticulum/network/nomadnet.html).
///
/// Produces the text format consumed by NomadNet clients (MeshChat, etc.).
/// All methods return `&mut Self` for chaining. Call [`build`](Self::build)
/// to produce the final markup string.
pub struct MicronBuilder {
    inner: String,
    directives_written: bool,
}

impl MicronBuilder {
    pub fn new() -> Self {
        Self {
            inner: String::new(),
            directives_written: false,
        }
    }

    fn ensure_directives(&mut self) {
        if !self.directives_written {
            self.directives_written = true;
        }
    }

    pub fn cache_directive(&mut self, seconds: u32) -> &mut Self {
        writeln!(self.inner, "#!c={seconds}").unwrap();
        self.directives_written = true;
        self
    }

    pub fn bg_color_directive(&mut self, hex: &str) -> &mut Self {
        writeln!(self.inner, "#!bg={hex}").unwrap();
        self.directives_written = true;
        self
    }

    pub fn fg_color_directive(&mut self, hex: &str) -> &mut Self {
        writeln!(self.inner, "#!fg={hex}").unwrap();
        self.directives_written = true;
        self
    }

    pub fn heading(&mut self, level: usize, text: &str) -> &mut Self {
        self.ensure_directives();
        let markers = ">".repeat(level.min(8));
        writeln!(self.inner, "{markers} {text}").unwrap();
        self
    }

    pub fn reset_depth(&mut self) -> &mut Self {
        self.ensure_directives();
        writeln!(self.inner, "<").unwrap();
        self
    }

    pub fn divider(&mut self) -> &mut Self {
        self.ensure_directives();
        writeln!(self.inner, "-").unwrap();
        self
    }

    pub fn custom_divider(&mut self, ch: char) -> &mut Self {
        self.ensure_directives();
        writeln!(self.inner, "-{ch}").unwrap();
        self
    }

    pub fn text(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        let escaped = Self::escape(text);
        writeln!(self.inner, "{escaped}").unwrap();
        self
    }

    pub fn text_raw_line(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        writeln!(self.inner, "{text}").unwrap();
        self
    }

    pub fn bold(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        let escaped = Self::escape(text);
        write!(self.inner, "`!{escaped}!").unwrap();
        self
    }

    pub fn italic(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        let escaped = Self::escape(text);
        write!(self.inner, "`*{escaped}*").unwrap();
        self
    }

    pub fn underline(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        let escaped = Self::escape(text);
        write!(self.inner, "`_{escaped}_").unwrap();
        self
    }

    pub fn color_fg(&mut self, hex: &str) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "`F{hex}").unwrap();
        self
    }

    pub fn color_bg(&mut self, hex: &str) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "`B{hex}").unwrap();
        self
    }

    pub fn reset_fg(&mut self) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "`f").unwrap();
        self
    }

    pub fn reset_bg(&mut self) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "`b").unwrap();
        self
    }

    pub fn reset_formatting(&mut self) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "` ").unwrap();
        self
    }

    pub fn center(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        let escaped = Self::escape(text);
        write!(self.inner, "`c{escaped}").unwrap();
        self
    }

    pub fn left_align(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        let escaped = Self::escape(text);
        write!(self.inner, "`l{escaped}").unwrap();
        self
    }

    pub fn right_align(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        let escaped = Self::escape(text);
        write!(self.inner, "`r{escaped}").unwrap();
        self
    }

    pub fn link(&mut self, label: &str, url: &str) -> &mut Self {
        self.ensure_directives();
        let escaped_label = Self::escape(label);
        write!(self.inner, "`[{escaped_label}`{url}]").unwrap();
        self
    }

    pub fn link_with_fields(&mut self, label: &str, url: &str, fields: &[&str]) -> &mut Self {
        self.ensure_directives();
        let escaped_label = Self::escape(label);
        if fields.is_empty() {
            write!(self.inner, "`[{escaped_label}`{url}]").unwrap();
        } else {
            let field_str = fields.join("|");
            write!(self.inner, "`[{escaped_label}`{url}`{field_str}]").unwrap();
        }
        self
    }

    pub fn lxmf_link(&mut self, label: &str, dest_hash: &str) -> &mut Self {
        self.ensure_directives();
        let escaped_label = Self::escape(label);
        write!(self.inner, "`[{escaped_label}`@lxmf:{dest_hash}]").unwrap();
        self
    }

    pub fn field(&mut self, name: &str, default: &str) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "`<{name}`{default}>").unwrap();
        self
    }

    pub fn field_with_width(&mut self, width: usize, name: &str, default: &str) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "`<{width}|{name}`{default}>").unwrap();
        self
    }

    pub fn masked_field(&mut self, name: &str, default: &str) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "`<!{name}`{default}>").unwrap();
        self
    }

    pub fn checkbox(&mut self, name: &str, value: &str, label: &str, checked: bool) -> &mut Self {
        self.ensure_directives();
        let check = if checked { "*" } else { "" };
        let escaped_label = Self::escape(label);
        write!(self.inner, "`<?|{name}|{value}|{check}> {escaped_label}").unwrap();
        self
    }

    pub fn submit_link(&mut self, label: &str, url: &str) -> &mut Self {
        self.link_with_fields(label, url, &["*"])
    }

    pub fn comment(&mut self, text: &str) -> &mut Self {
        writeln!(self.inner, "# {text}").unwrap();
        self
    }

    pub fn literal(&mut self, text: &str) -> &mut Self {
        self.ensure_directives();
        writeln!(self.inner, "`={text}").unwrap();
        self
    }

    pub fn blank_line(&mut self) -> &mut Self {
        writeln!(self.inner).unwrap();
        self
    }

    pub fn raw(&mut self, micron: &str) -> &mut Self {
        self.ensure_directives();
        write!(self.inner, "{micron}").unwrap();
        self
    }

    pub fn build(&self) -> String {
        self.inner.clone()
    }

    pub fn escape(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for ch in input.chars() {
            match ch {
                '`' => out.push_str("``"),
                _ => out.push(ch),
            }
        }
        out
    }
}

impl Default for MicronBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MicronBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading() {
        let mut b = MicronBuilder::new();
        b.heading(1, "Server Info");
        assert!(b.build().contains("> Server Info\n"));
    }

    #[test]
    fn test_nested_heading() {
        let mut b = MicronBuilder::new();
        b.heading(2, "Sub");
        assert!(b.build().contains(">> Sub\n"));
    }

    #[test]
    fn test_divider() {
        let mut b = MicronBuilder::new();
        b.divider();
        assert!(b.build().contains("-\n"));
    }

    #[test]
    fn test_bold() {
        let mut b = MicronBuilder::new();
        b.bold("hello");
        assert!(b.build().contains("`!hello!"));
    }

    #[test]
    fn test_italic() {
        let mut b = MicronBuilder::new();
        b.italic("hello");
        assert!(b.build().contains("`*hello*"));
    }

    #[test]
    fn test_underline() {
        let mut b = MicronBuilder::new();
        b.underline("hello");
        assert!(b.build().contains("`_hello_"));
    }

    #[test]
    fn test_link() {
        let mut b = MicronBuilder::new();
        b.link("Go", "abc123:/page/index.mu");
        assert!(b.build().contains("`[Go`abc123:/page/index.mu]"));
    }

    #[test]
    fn test_lxmf_link() {
        let mut b = MicronBuilder::new();
        b.lxmf_link("Chat", "deadbeef12345678deadbeef12345678");
        assert!(b
            .build()
            .contains("`[Chat`@lxmf:deadbeef12345678deadbeef12345678]"));
    }

    #[test]
    fn test_field() {
        let mut b = MicronBuilder::new();
        b.field("username", "");
        assert!(b.build().contains("`<username`>"));
    }

    #[test]
    fn test_checkbox() {
        let mut b = MicronBuilder::new();
        b.checkbox("agree", "yes", "I agree", false);
        assert!(b.build().contains("`<?|agree|yes|> I agree"));
    }

    #[test]
    fn test_checkbox_checked() {
        let mut b = MicronBuilder::new();
        b.checkbox("agree", "yes", "I agree", true);
        assert!(b.build().contains("`<?|agree|yes|*> I agree"));
    }

    #[test]
    fn test_submit_link() {
        let mut b = MicronBuilder::new();
        b.submit_link("Send", "abc123:/page/index.mu");
        assert!(b.build().contains("`[Send`abc123:/page/index.mu`*]"));
    }

    #[test]
    fn test_cache_directive() {
        let mut b = MicronBuilder::new();
        b.cache_directive(0);
        assert!(b.build().starts_with("#!c=0\n"));
    }

    #[test]
    fn test_color_directives() {
        let mut b = MicronBuilder::new();
        b.bg_color_directive("222");
        b.fg_color_directive("eee");
        let s = b.build();
        assert!(s.contains("#!bg=222\n"));
        assert!(s.contains("#!fg=eee\n"));
    }

    #[test]
    fn test_color_inline() {
        let mut b = MicronBuilder::new();
        b.color_fg("f00");
        assert!(b.build().contains("`Ff00"));
    }

    #[test]
    fn test_escape() {
        assert_eq!(MicronBuilder::escape("hello `world`"), "hello ``world``");
    }

    #[test]
    fn test_escape_empty() {
        assert_eq!(MicronBuilder::escape(""), "");
    }

    #[test]
    fn test_comment() {
        let mut b = MicronBuilder::new();
        b.comment("this is hidden");
        assert!(b.build().contains("# this is hidden\n"));
    }

    #[test]
    fn test_literal() {
        let mut b = MicronBuilder::new();
        b.literal("`!not bold!`");
        assert!(b.build().contains("`=`!not bold!`\n"));
    }

    #[test]
    fn test_blank_line() {
        let mut b = MicronBuilder::new();
        b.text("hello");
        b.blank_line();
        b.text("world");
        assert_eq!(b.build(), "hello\n\nworld\n");
    }

    #[test]
    fn test_full_page() {
        let mut b = MicronBuilder::new();
        b.cache_directive(3600);
        b.bg_color_directive("111");
        b.heading(1, "LXIRC Server");
        b.divider();
        b.bold("Users online: ");
        b.text("3");
        b.blank_line();
        b.heading(2, "Channels");
        b.link("#general", "abc123:/page/channels/general.mu");
        b.blank_line();
        let page = b.build();
        assert!(page.starts_with("#!c=3600\n#!bg=111\n"));
        assert!(page.contains("> LXIRC Server\n"));
        assert!(page.contains("-\n"));
        assert!(page.contains("`!Users online: !"));
        assert!(page.contains("3\n"));
        assert!(page.contains(">> Channels\n"));
    }

    #[test]
    fn test_display_impl() {
        let mut b = MicronBuilder::new();
        b.heading(1, "Test");
        let display = format!("{b}");
        assert_eq!(display, "> Test\n");
    }
}
