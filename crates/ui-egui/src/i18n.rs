//! Interface languages. Strings in the code stay English and are the lookup keys; the catalogs map
//! them to display text when the UI is drawn. Command ids, document text, file names, the control
//! channel, the CLI and MCP never see translated text, so scripts and agents are unaffected. A
//! string without a translation shows in English, so coverage can grow one entry at a time and a
//! missing translation never fails a build, a test or a release.
//!
//! # Where the catalogs live
//! Unlike WordCraft, DeckCraft has no `deckcraft.tsv` in storytold/craft-i18n yet, so the fork keeps
//! its catalogs in the repository: `i18n/locales/<locale>/common.tsv` (a copy of craft-i18n's shared
//! file; its README line names the commit) and `i18n/locales/<locale>/deckcraft.tsv` (DeckCraft's
//! own rows). `build.rs` embeds them; `CRAFT_I18N_DIR` (a craft-i18n checkout) is read first when
//! set. No setup is needed. Lookup, fallback (`pt-br` → `pt` → English, app rows over shared ones),
//! plurals, language names and locale matching are the `craft-i18n` crate's (craft-libs).
//!
//! The `language` setting (`ui.json`) is `auto` by default: the UI follows the operating system's
//! language when there is a catalog for it, and English otherwise. Preferences ▸ Interface language
//! picks one by hand; `DECKCRAFT_LANGUAGE=pt-BR` overrides the system for `auto`.
//!
//! # How to add strings (read this before wrapping more of the UI)
//! 1. The English literal is the key and stays in the code. Wrap what is *drawn* with
//!    [`tl!`](crate::tl) (`ui.button(tl!("Paste"))`, `.on_hover_text(tl!("Cut"))`). Never wrap
//!    command ids, parameters, control-channel names, state keys (`app.ui.tab == "Home"`), file text
//!    or anything that is saved: translate at the moment of painting only.
//! 2. Text with a runtime part: translate the template, then [`fmt`] it, with named placeholders:
//!    `fmt(tl!("Slide {n} of {total}"), &[("n", &n.to_string()), ("total", &total.to_string())])`.
//!    Never `format!` an English sentence and translate the result. Counts: [`tn`].
//! 3. Same English, different meaning: [`tc`] with a context (`tc("verb", "Open")`) and a row with
//!    that context in column one.
//! 4. Shared widgets (`widgets.rs`: big and small buttons, `icon_toggle`, `check_item`, `dropdown`,
//!    `color_split`, `section`, `spinner`) translate their label themselves, so a call such as
//!    `small(app, ui, Icon::Cut, "Cut", "edit.cut", json!({}))` needs no wrap. Do not wrap twice.
//! 5. Rows go in `i18n/locales/pt-br/deckcraft.tsv`: `context TAB English TAB translation`, sorted
//!    by English. If the English already has a row in `common.tsv` do not repeat it. The
//!    `deckcraft_rows_parse` test checks the file; `catalog_covers_*` tests list what is missing.
//! 6. Tests drive the UI by English labels: `system_lang()` is English under `cfg!(test)`, so a test
//!    that wants Portuguese sets `app.ui.language = "pt-br"` explicitly.
//!
//! # Adding a language
//! Add `i18n/locales/<code>/{common,deckcraft}.tsv` (the `@id language.name` row names it) and one
//! row to [`LANGUAGES`]. The Preferences list and the system-language match pick it up.
//!
//! # Looking strings up
//! - [`tl!`](crate::tl) / [`t`]: a string in the current language. [`tr`]: in a given one.
//! - [`tc`] / [`trc`]: worded for a context, [`get`]: that context's row only.
//! - [`fmt`]: fill `{name}` placeholders after a lookup; translators may reorder them.
//! - [`fmt_plain`]: [`fmt`] without bidi isolate marks, for text that goes into the document.

use std::cell::Cell;
use std::sync::{LazyLock, OnceLock};

use craft_i18n::I18n;

pub use craft_i18n::{fmt, fmt_plain};

include!(concat!(env!("OUT_DIR"), "/craft_i18n.rs"));

/// The embedded catalogs, offering exactly the languages in [`LANGUAGES`]. Built on first use;
/// a locale is parsed on its first lookup.
static I18N: LazyLock<I18n> = LazyLock::new(|| {
    let offered: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
    let (i18n, problems) = I18n::new(CRAFT_I18N, &offered);
    problems.iter().for_each(|p| log::warn!("interface catalogs: {p}"));
    i18n
});

/// The `language` setting that follows the system.
pub const AUTO: &str = "auto";

/// One interface language DeckCraft offers. Its name and translations come from the catalogs.
pub struct LangInfo {
    /// BCP 47 code, lowercase (`pt-br`). Also the `language` setting's value and the locale.
    pub code: &'static str,
}

/// The languages DeckCraft offers, when their catalogs are embedded. English first: it is the
/// fallback and the source language.
pub static LANGUAGES: [LangInfo; 2] = [
    LangInfo { code: "en" },
    // Brazilian Portuguese; `pt-BR` and `pt-BR-*` resolve here, and plain `pt` through the crate's
    // matching.
    LangInfo { code: "pt-br" },
];

/// A language the UI can be shown in: English, or an offered language whose catalogs are embedded.
#[derive(Clone, Copy)]
pub struct Lang {
    lang: craft_i18n::Lang,
    /// The catalogs it is looked up in: the embedded ones ([`I18N`]), or a test fixture's.
    catalogs: Option<&'static I18n>,
}

impl std::fmt::Debug for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lang({})", self.code())
    }
}

impl PartialEq for Lang {
    fn eq(&self, other: &Self) -> bool {
        self.code() == other.code()
    }
}

impl Eq for Lang {}

impl Lang {
    pub const EN: Lang = Lang { lang: craft_i18n::Lang::EN, catalogs: None };

    fn i18n(self) -> &'static I18n {
        self.catalogs.unwrap_or_else(|| LazyLock::force(&I18N))
    }

    pub fn code(self) -> &'static str {
        self.i18n().code(self.lang)
    }

    /// The language's name in itself (its `@id language.name` row), shown in the Preferences list.
    pub fn name(self) -> &'static str {
        self.i18n().name(self.lang)
    }

    /// A language by its exact code (any case): one of [`LANGUAGES`] whose catalogs are embedded.
    pub fn from_code(code: &str) -> Option<Lang> {
        let info = LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code))?;
        I18N.from_code(info.code).map(|lang| Lang { lang, catalogs: None })
    }

    /// Resolve the `language` setting: a language code, or [`AUTO`] (and anything unknown, e.g. a
    /// code from a newer version) to follow the system.
    pub fn from_pref(pref: &str) -> Lang {
        Lang::from_code(pref).unwrap_or_else(system_lang)
    }

    /// Every language the interface can be shown in: [`LANGUAGES`] in order, those with embedded
    /// catalogs (English only in a build without any).
    pub fn all() -> impl Iterator<Item = Lang> {
        LANGUAGES.iter().filter_map(|l| Lang::from_code(l.code))
    }
}

/// The canonical `language` setting for user input: [`AUTO`] or a registered code (any case).
/// `None` for anything else, so callers keep the current setting.
pub fn normalize_pref(pref: &str) -> Option<&'static str> {
    if pref.eq_ignore_ascii_case(AUTO) {
        return Some(AUTO);
    }
    Lang::from_code(pref).map(Lang::code)
}

/// The registered language for a locale tag such as `pt_BR.UTF-8`; `None` when there is no catalog
/// for it. `C`/`POSIX` mean English.
pub fn lang_from_tag(tag: &str) -> Option<Lang> {
    I18N.match_tag(tag).and_then(|lang| Lang::from_code(I18N.code(lang)))
}

/// The first of the system's preferred languages DeckCraft has, in the user's order.
pub fn first_supported<'a>(tags: impl IntoIterator<Item = &'a str>) -> Option<Lang> {
    tags.into_iter().find_map(lang_from_tag)
}

/// The system language (looked up once). English when it can't be determined or isn't supported.
pub fn system_lang() -> Lang {
    // Tests drive the UI by its English labels whatever the machine's language is.
    if cfg!(test) {
        return Lang::EN;
    }
    static SYSTEM: OnceLock<Lang> = OnceLock::new();
    *SYSTEM.get_or_init(detect_system_lang)
}

fn detect_system_lang() -> Lang {
    // An explicit override, then the platform's preferred UI languages (Windows display language,
    // macOS preferred languages, `LC_ALL`/`LC_MESSAGES`/`LANG` on Unix, the browser on the web).
    if let Some(l) = std::env::var("DECKCRAFT_LANGUAGE").ok().and_then(|v| lang_from_tag(&v)) {
        return l;
    }
    Lang::from_code(I18N.code(I18N.system_lang())).unwrap_or(Lang::EN)
}

thread_local! {
    // Per thread: parallel test harnesses must not change each other's language.
    static CURRENT: Cell<Lang> = const { Cell::new(Lang::EN) };
}

/// Set the language the UI is drawn in (the app does this every frame from its setting).
pub fn set_current(lang: Lang) {
    CURRENT.set(lang);
}

/// The language the UI is drawn in.
pub fn current() -> Lang {
    CURRENT.get()
}

/// Does `lang` have a translation for this string? (English never does: it is the source.)
pub fn has(lang: Lang, s: &str) -> bool {
    lang.i18n().get(lang.lang, "", s).is_some()
}

/// `s` in the current language ([`tr`] with [`current`]).
pub fn t(s: &str) -> &str {
    tr(current(), s)
}

/// Lowercase for searching translated labels (Turkish is not offered, so this is Unicode's rule).
pub fn lowercase(s: &str) -> String {
    s.to_lowercase()
}

/// `s` in `lang`; strings without a translation come back unchanged.
pub fn tr(lang: Lang, s: &str) -> &str {
    lang.i18n().t(lang.lang, s)
}

/// A two-line ribbon label (`New\nSlide`) in the current language. A row for the one-line text
/// (`New Slide`) is enough: the button breaks it near the middle itself.
pub fn t_lines(s: &str) -> std::borrow::Cow<'_, str> {
    let direct = t(s);
    if direct != s || !s.contains('\n') {
        return std::borrow::Cow::Borrowed(direct);
    }
    let one_line = s.replace('\n', " ");
    let translated = t(&one_line);
    if translated != one_line { std::borrow::Cow::Owned(translated.to_string()) } else { std::borrow::Cow::Borrowed(s) }
}

/// A count's wording in the current language: `one` and `other` are the English forms with `{n}`
/// for the count, and the key of the catalog's `@plural` row.
pub fn tn(n: u64, one: &str, other: &str) -> String {
    let lang = current();
    lang.i18n().trn(lang.lang, n, one, other)
}

/// `s` in the current language as worded for `context`, falling back to the plain translation and
/// then to English.
pub fn tc<'a>(context: &str, s: &'a str) -> &'a str {
    trc(current(), context, s)
}

/// [`tc`] in a given language.
pub fn trc<'a>(lang: Lang, context: &str, s: &'a str) -> &'a str {
    lang.i18n().tc(lang.lang, context, s)
}

/// `s`'s translation for `context` in the current language, with no fallback to the plain row or
/// English: for a word whose plain translation means something else here.
pub fn get<'a>(context: &str, s: &'a str) -> Option<&'a str> {
    let lang = current();
    lang.i18n().get(lang.lang, context, s)
}

/// A label with an action after a fixed English prefix, e.g. `Undo Typing`: both parts translated
/// where the catalog has them (`@msg`-less rows `Undo {action}` in the catalog).
pub fn prefixed(prefix: &str, rest: &str) -> String {
    if rest.is_empty() {
        return t(prefix).to_string();
    }
    fmt(t(&format!("{prefix} {{action}}")), &[("action", t(rest))])
}

/// A test language whose catalog is `source` (catalog format), so tests of lookup code don't
/// depend on the embedded catalogs' wording.
#[cfg(test)]
pub(crate) fn fixture_lang(code: &'static str, source: &'static str) -> Lang {
    let embedded: &'static craft_i18n::Embedded = Box::leak(Box::new([(code, "", source)]));
    let (i18n, _) = I18n::new(embedded, &[code]);
    let i18n: &'static I18n = Box::leak(Box::new(i18n));
    let lang = i18n.from_code(code).unwrap_or(craft_i18n::Lang::EN);
    Lang { lang, catalogs: Some(i18n) }
}

/// For a test that needs the embedded catalogs: `false`, with a note, when none were embedded, so
/// the test can skip; the interface is then English only.
#[cfg(test)]
pub(crate) fn catalogs_embedded() -> bool {
    if CRAFT_I18N.is_empty() {
        eprintln!("skipped: built without interface catalogs; the interface is English only");
    }
    !CRAFT_I18N.is_empty()
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_backstage;
