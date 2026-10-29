//! System fonts for the characters Inter does not have: Japanese, Chinese,
//! Korean, and symbols such as `❥`. Without them those titles draw as boxes.
//!
//! [`install`] looks the fonts up with `fc-match` on a background thread at
//! startup, memory-maps the files, and hands each to egui as a
//! [`FontProvider`]. egui asks the providers only for a character no
//! installed font has, and appends the font it gets to the end of that
//! family's chain, so Inter and Phosphor always win. Nothing is bundled.
//!
//! Costs: a library without such characters never parses a fallback font,
//! and a mapped font's pages count toward RSS only once read, so a 33 MB
//! Noto Sans CJK costs about what the glyphs on screen need. A system with
//! none of these fonts, or no fontconfig, starts exactly as before.
//!
//! Color emoji fonts are skipped: egui's `color_fonts` feature is off, so
//! they would draw nothing. Emoji come from a monochrome font when one is
//! installed (DejaVu Sans covers many).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::{FallbackRequest, FontData, FontInsert, FontProvider, FontTweak};

use crate::theme;

/// fontconfig patterns for the CJK fonts, each with the language the
/// returned font must support. Asking for each script finds a second font
/// when the first lacks one (a Japanese-only font has no Hangul).
const CJK_PATTERNS: [(&str, &str); 3] = [
    ("sans-serif:lang=ja", "ja"),
    ("sans-serif:lang=ko", "ko"),
    ("sans-serif:lang=zh-cn", "zh-cn"),
];

/// Monochrome symbol and emoji families, best first. fontconfig answers any
/// name with its default font, so the answer must be the family asked for.
const SYMBOL_FAMILIES: [&str; 3] = ["Noto Emoji", "Symbola", "DejaVu Sans"];

/// Where Fedora, Debian and Arch put the same fonts, for a system without
/// `fc-match`. CJK first, then symbols, as with fontconfig.
const KNOWN_PATHS: [&str; 9] = [
    "/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc",
    "/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-droid-sans-fonts/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];

/// `fc-match` output: path, face index, color flag, languages, families.
const FC_FORMAT: &str = "%{file}\t%{index}\t%{color}\t%{lang}\t%{family}";

/// Finds the fallback fonts on a background thread and installs them.
/// Returns at once; until the thread finishes (tens of ms) missing glyphs
/// draw as boxes, then egui reloads its fonts and redraws.
pub fn install(ctx: &egui::Context) {
    let ctx = ctx.clone();
    // No thread: no fallback, as on a system without the fonts.
    let _ = std::thread::Builder::new()
        .name("rustle-fonts".into())
        .spawn(move || {
            let providers: Vec<Arc<dyn FontProvider>> = find()
                .iter()
                .enumerate()
                .filter_map(|(i, font)| SystemFont::open(i, font))
                .map(|font| Arc::new(font) as Arc<dyn FontProvider>)
                .collect();
            if !providers.is_empty() {
                // eframe's own provider is off (`system_font_fallback`), so
                // this replaces nothing.
                ctx.set_font_providers(providers);
            }
        });
}

/// One font face in a font file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    path: PathBuf,
    /// The face in a `.ttc` collection; 0 for a single font.
    index: u32,
}

/// The fallback fonts, CJK first. Empty if there are none.
fn find() -> Vec<Found> {
    let mut found = Vec::new();
    let mut fc_ran = false;
    for (pattern, lang) in CJK_PATTERNS {
        if let Some(out) = fc_match(pattern) {
            fc_ran = true;
            found.extend(parse_match(&out, Want::Lang(lang)));
        }
    }
    for family in SYMBOL_FAMILIES {
        let Some(out) = fc_match(family) else {
            continue;
        };
        fc_ran = true;
        if let Some(font) = parse_match(&out, Want::Family(family)) {
            found.push(font);
            break;
        }
    }
    if !fc_ran {
        found = KNOWN_PATHS
            .iter()
            .map(Path::new)
            .filter(|p| p.is_file())
            .map(|p| Found {
                path: p.to_owned(),
                index: 0,
            })
            .collect();
    }
    dedup_files(found)
}

/// Runs `fc-match` for one pattern. `None` if it cannot run or fails.
fn fc_match(pattern: &str) -> Option<String> {
    let out = Command::new("fc-match")
        .args(["--format", FC_FORMAT, pattern])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What an `fc-match` answer must have to be used.
#[derive(Debug, Clone, Copy)]
enum Want<'a> {
    /// Support for this fontconfig language, such as `ja`.
    Lang(&'a str),
    /// This family name.
    Family(&'a str),
}

/// Reads one [`FC_FORMAT`] line. `None` for a color font, or one that is
/// only fontconfig's best guess (no such language, a different family).
fn parse_match(out: &str, want: Want<'_>) -> Option<Found> {
    let mut fields = out.trim_end_matches('\n').split('\t');
    let path = fields.next().filter(|p| !p.is_empty())?;
    // The high 16 bits name a variable font's instance; the weight comes
    // from the family instead (`SystemFont::font_for`).
    let index = fields.next()?.parse::<u32>().ok()? & 0xFFFF;
    let color = fields.next()?;
    let langs = fields.next()?;
    let families = fields.next()?;
    if color.eq_ignore_ascii_case("true") {
        return None;
    }
    let ok = match want {
        Want::Lang(lang) => langs.split('|').any(|l| l == lang),
        Want::Family(family) => families
            .split(',')
            .any(|f| f.trim().eq_ignore_ascii_case(family)),
    };
    ok.then(|| Found {
        path: PathBuf::from(path),
        index,
    })
}

/// Keeps the first face of each file: one face of Noto Sans CJK or Droid
/// Sans Fallback covers Japanese, Chinese and Korean.
fn dedup_files(found: Vec<Found>) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::with_capacity(found.len());
    for font in found {
        if !out.iter().any(|f| f.path == font.path) {
            out.push(font);
        }
    }
    out
}

/// A fallback font, given to egui on request.
struct SystemFont {
    /// Unique per font; a weight is appended for a variable font.
    name: String,
    data: FontData,
    /// Has a `wght` axis: one instance per text weight, sharing the bytes.
    variable: bool,
}

impl SystemFont {
    /// Maps the font. `None` if the file cannot be read.
    fn open(n: usize, font: &Found) -> Option<Self> {
        let bytes = Mapped::open(&font.path).ok()?;
        let data = FontData::from_blob(Arc::new(bytes), font.index);
        let variable = data
            .variation_axes()
            .iter()
            .any(|axis| axis.tag.to_be_bytes() == *b"wght");
        Some(Self {
            name: format!("system-fallback-{n}"),
            data,
            variable,
        })
    }
}

impl FontProvider for SystemFont {
    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        // Text families only: an icon family never falls back to text.
        let weight = theme::text_weight(request.family)?;
        if !self.variable {
            return Some(FontInsert::new(&self.name, self.data.clone(), Vec::new()));
        }
        // Noto Sans CJK VF defaults to Thin; match Inter's weight instead.
        let tweak = FontTweak {
            coords: VariationCoords::new([(b"wght", weight)]),
            ..FontTweak::default()
        };
        Some(FontInsert::new(
            &format!("{}-{weight}", self.name),
            self.data.clone().tweak(tweak),
            Vec::new(),
        ))
    }
}

/// A read-only memory map of a whole file. Its pages are read, and count
/// toward RSS, only when touched, and the kernel can drop them again.
struct Mapped {
    ptr: *const u8,
    len: usize,
}

// SAFETY: the mapping is read-only and owned by this value; sharing reads
// across threads is fine.
unsafe impl Send for Mapped {}
unsafe impl Sync for Mapped {}

#[cfg(unix)]
mod sys {
    use std::ffi::{c_int, c_void};

    pub const PROT_READ: c_int = 1;
    pub const MAP_PRIVATE: c_int = 2;
    pub const MAP_FAILED: *mut c_void = !0 as *mut c_void;

    unsafe extern "C" {
        pub fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: i64,
        ) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
    }
}

impl Mapped {
    #[cfg(unix)]
    fn open(path: &Path) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        let file = File::open(path)?;
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| std::io::Error::other("font file too large"))?;
        if len == 0 {
            return Err(std::io::Error::other("empty font file"));
        }
        // SAFETY: a fresh private read-only mapping of an open file; the fd
        // may close afterwards. A file truncated while mapped would fault on
        // read, but package managers replace fonts with a new file (rename),
        // which leaves this mapping intact.
        let ptr = unsafe {
            sys::mmap(
                std::ptr::null_mut(),
                len,
                sys::PROT_READ,
                sys::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == sys::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr.cast_const().cast(),
            len,
        })
    }

    #[cfg(not(unix))]
    fn open(_path: &Path) -> std::io::Result<Self> {
        Err(std::io::Error::other("no memory map on this platform"))
    }
}

impl AsRef<[u8]> for Mapped {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: `ptr` maps `len` readable bytes until `drop`.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: unmaps exactly the mapping made in `open`, once.
        unsafe {
            sys::munmap(self.ptr.cast_mut().cast(), self.len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{FontDefinitions, FontFamily, FontId};

    const CJK_VF: &str = "/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc";
    const INTER: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/fonts/Inter-Regular.ttf"
    );
    const PHOSPHOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/Phosphor.ttf");

    fn found(path: &str, index: u32) -> Found {
        Found {
            path: PathBuf::from(path),
            index,
        }
    }

    #[test]
    fn reads_a_cjk_answer_and_drops_the_instance_bits() {
        // fontconfig's answer for `sans-serif:lang=ko` on Nobara 44.
        let out = format!(
            "{CJK_VF}\t262145\tFalse\taa|ja|ko|zh-cn|zh-tw\tNoto Sans CJK KR,Noto Sans CJK KR Regular"
        );
        assert_eq!(parse_match(&out, Want::Lang("ko")), Some(found(CJK_VF, 1)));
    }

    #[test]
    fn skips_a_default_font_without_the_language() {
        // No CJK font installed: fontconfig still answers with its default.
        let out =
            "/usr/share/fonts/dejavu/DejaVuSans.ttf\t0\tFalse\taa|af|ru|zh-hk-ish\tDejaVu Sans";
        assert_eq!(parse_match(out, Want::Lang("ja")), None);
        assert_eq!(parse_match(out, Want::Lang("zh-cn")), None);
    }

    #[test]
    fn skips_color_fonts() {
        let out = "/usr/share/fonts/emoji/Noto-COLRv1.ttf\t0\tTrue\tund-zsye\tNoto Color Emoji";
        assert_eq!(parse_match(out, Want::Family("Noto Color Emoji")), None);
    }

    #[test]
    fn a_symbol_font_must_be_the_family_asked_for() {
        // `fc-match "Noto Emoji"` without it installed.
        let out = "/usr/share/fonts/noto/NotoSans-Regular.ttf\t0\tFalse\taa|en\tNoto Sans";
        assert_eq!(parse_match(out, Want::Family("Noto Emoji")), None);
        let out =
            "/usr/share/fonts/dejavu/DejaVuSans.ttf\t0\tFalse\taa|en\tDejaVu Sans,DejaVu Sans Book";
        assert_eq!(
            parse_match(out, Want::Family("DejaVu Sans")),
            Some(found("/usr/share/fonts/dejavu/DejaVuSans.ttf", 0))
        );
    }

    #[test]
    fn malformed_answers_are_skipped() {
        for out in [
            "",
            "\n",
            "/a.ttf",
            "/a.ttf\tx\tFalse\tja\tA",
            "\t0\tFalse\tja\tA",
        ] {
            assert_eq!(parse_match(out, Want::Lang("ja")), None, "{out:?}");
        }
    }

    #[test]
    fn keeps_one_face_per_file() {
        let fonts = vec![found(CJK_VF, 0), found(CJK_VF, 1), found("/b.ttf", 0)];
        assert_eq!(
            dedup_files(fonts),
            vec![found(CJK_VF, 0), found("/b.ttf", 0)]
        );
    }

    #[test]
    fn maps_a_file_byte_for_byte() {
        let mapped = Mapped::open(Path::new(INTER)).unwrap();
        assert!(mapped.as_ref() == include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    }

    #[test]
    fn an_empty_or_missing_file_is_an_error() {
        let empty = std::env::temp_dir().join(format!("rustle-empty-{}.ttf", std::process::id()));
        File::create(&empty).unwrap();
        assert!(Mapped::open(&empty).is_err());
        std::fs::remove_file(&empty).unwrap();
        assert!(Mapped::open(Path::new("/nonexistent/font.ttf")).is_err());
    }

    fn request<'a>(family: &'a FontFamily) -> FallbackRequest<'a> {
        FallbackRequest {
            cluster: "한",
            family,
        }
    }

    #[test]
    fn a_static_font_is_one_face_for_every_text_family() {
        let font = SystemFont::open(3, &found(INTER, 0)).unwrap();
        assert!(!font.variable);
        for family in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name(theme::INTER_600.into()),
        ] {
            let insert = font.font_for(&request(&family)).unwrap();
            assert_eq!(insert.name, "system-fallback-3");
            assert_eq!(insert.data.tweak.coords, VariationCoords::default());
        }
        let icons = FontFamily::Name(theme::PHOSPHOR.into());
        assert!(font.font_for(&request(&icons)).is_none());
    }

    #[test]
    fn a_variable_font_matches_each_family_weight() {
        let mut font = SystemFont::open(0, &found(INTER, 0)).unwrap();
        font.variable = true;
        for (family, weight) in [
            (FontFamily::Proportional, 400.0),
            (FontFamily::Name(theme::INTER_500.into()), 500.0),
            (FontFamily::Name(theme::INTER_600.into()), 600.0),
        ] {
            let insert = font.font_for(&request(&family)).unwrap();
            assert_eq!(insert.name, format!("system-fallback-0-{weight}"));
            assert_eq!(
                insert.data.tweak.coords,
                VariationCoords::new([(b"wght", weight)])
            );
        }
    }

    /// A context with Inter alone in the text families, so Phosphor's icons
    /// stand in for a script Inter lacks.
    fn inter_only(providers: Vec<Arc<dyn FontProvider>>) -> egui::Context {
        let ctx = egui::Context::default();
        let mut defs = FontDefinitions::empty();
        defs.font_data.insert(
            "inter".into(),
            Arc::new(FontData::from_static(include_bytes!(
                "../assets/fonts/Inter-Regular.ttf"
            ))),
        );
        for family in [
            FontFamily::Proportional,
            FontFamily::Name(theme::INTER_600.into()),
        ] {
            defs.families.insert(family, vec!["inter".into()]);
        }
        ctx.set_fonts(defs);
        ctx.set_font_providers(providers);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        ctx
    }

    #[test]
    fn a_provided_font_fills_glyphs_the_chain_lacks() {
        let icon = crate::icons::X.chars().next().unwrap();
        let ctx = inter_only(Vec::new());
        assert!(!ctx.fonts_mut(|f| f.has_glyph(&FontId::proportional(14.0), icon)));

        let phosphor = SystemFont::open(0, &found(PHOSPHOR, 0)).unwrap();
        let ctx = inter_only(vec![Arc::new(phosphor)]);
        ctx.fonts_mut(|f| {
            assert!(f.has_glyph(&FontId::proportional(14.0), icon));
            let semibold = FontId::new(14.0, FontFamily::Name(theme::INTER_600.into()));
            assert!(f.has_glyph(&semibold, icon));
            // Inter still draws what it has.
            assert!(f.has_glyph(&FontId::proportional(14.0), 'a'));
        });
    }

    #[test]
    fn a_broken_font_file_draws_boxes_without_panicking() {
        let junk = SystemFont::open(
            0,
            &found(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"), 0),
        )
        .unwrap();
        let ctx = inter_only(vec![Arc::new(junk)]);
        assert!(!ctx.fonts_mut(|f| f.has_glyphs(&FontId::proportional(14.0), "한❥")));
        ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("가사해석 ❥attack");
        })
        .textures_delta
        .clear();
    }

    /// The real lookup on this machine. Checks only what the installed fonts
    /// allow: on a system without a CJK font (CI) it checks nothing.
    #[test]
    fn system_fonts_cover_the_titles_from_issue_3() {
        let has_cjk = fc_match(CJK_PATTERNS[0].0)
            .and_then(|out| parse_match(&out, Want::Lang("ja")))
            .is_some();
        if !has_cjk {
            eprintln!("no CJK font on this system; skipping");
            return;
        }
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let providers: Vec<Arc<dyn FontProvider>> = find()
            .iter()
            .enumerate()
            .filter_map(|(i, f)| SystemFont::open(i, f))
            .map(|f| Arc::new(f) as Arc<dyn FontProvider>)
            .collect();
        ctx.set_font_providers(providers);
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        ctx.fonts_mut(|f| {
            for font in [theme::body(), theme::page_title()] {
                for title in [
                    "ヒカリアレ - Hikariare",
                    "가사해석",
                    "번역하는 공대생",
                    "中文",
                ] {
                    assert!(f.has_glyphs(&font, title), "{title:?} in {font:?}");
                }
            }
        });
    }
}
