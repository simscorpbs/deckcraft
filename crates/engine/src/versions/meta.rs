//! What a version records about how and where it was saved, without tracking a person.
//!
//! Only one thing goes into the presentation itself: a random document id ([`DOC_ID_PROP`], a
//! UUID v4 that derives from nothing: not the machine, the user, the time or the path). The
//! history's own records stay in the history folder and hold counts and hashes, never text of
//! the document. The computer appears as a random installation id and a label the user can
//! change ("Windows", "Mac" by default), never its host name or the account name; paths are
//! kept relative to the user's folder or the cloud provider's folder.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// The custom document property (ECMA-376 Part 1 §22.3, `docProps/custom.xml`) that holds the
/// document id. PowerPoint keeps custom properties when it saves (they are the Custom tab of
/// Advanced Properties), so the id survives a round trip through PowerPoint.
pub(crate) const DOC_ID_PROP: &str = "DeckCraftDocumentId";
/// The installation id's file in the settings folder.
const INSTALLATION_FILE: &str = "installation-id";
/// Largest `docProps/custom.xml` looked at for the id.
const MAX_CUSTOM_XML: u64 = 1 << 20;

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new document id: a random UUID (version 4, RFC 9562 §5.4), lower-case.
pub(crate) fn new_document_id() -> Result<String, String> {
    let mut b = random_bytes::<16>()?;
    if let Some(byte) = b.get_mut(6) {
        *byte = (*byte & 0x0f) | 0x40;
    }
    if let Some(byte) = b.get_mut(8) {
        *byte = (*byte & 0x3f) | 0x80;
    }
    let h = hex(&b);
    let part = |range: std::ops::Range<usize>| h.get(range).unwrap_or_default();
    Ok(format!("{}-{}-{}-{}-{}", part(0..8), part(8..12), part(12..16), part(16..20), part(20..32)))
}

/// Is `text` a document id as DeckCraft writes them (a UUID in its usual text form)? Ids come
/// from files, so anything else is ignored.
pub(crate) fn document_id_ok(text: &str) -> bool {
    text.len() == 36
        && text
            .bytes()
            .enumerate()
            .all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() && !b.is_ascii_uppercase() })
}

/// The history folder of a document id: its 32 hex digits.
pub(crate) fn id_bucket(id: &str) -> String {
    id.chars().filter(|c| *c != '-').collect()
}

/// The id in the text of a `docProps/custom.xml`, when it holds a valid one.
pub(crate) fn id_in_custom_xml(xml: &str) -> Option<String> {
    let at = xml.find(&format!("name=\"{DOC_ID_PROP}\"")).or_else(|| xml.find(&format!("name='{DOC_ID_PROP}'")))?;
    let rest = xml.get(at..)?;
    let end = rest.find("</property>").unwrap_or(rest.len());
    let property = rest.get(..end)?;
    // `<vt:lpwstr>` with or without attributes (a namespace declaration, say).
    let tag = property.find("lpwstr")?;
    let value_start = tag + property.get(tag..)?.find('>')? + 1;
    let value = property.get(value_start..)?.split('<').next()?.trim();
    document_id_ok(value).then(|| value.to_owned())
}

/// The document id in a presentation package, read from `docProps/custom.xml` without opening
/// the presentation. `None` for anything that is not a package with a valid id.
pub(crate) fn package_document_id<R: Read + std::io::Seek>(reader: R) -> Option<String> {
    let mut archive = zip::ZipArchive::new(reader).ok()?;
    let mut part = archive.by_name("docProps/custom.xml").ok()?;
    if part.size() > MAX_CUSTOM_XML {
        return None;
    }
    let mut xml = String::new();
    part.by_ref().take(MAX_CUSTOM_XML).read_to_string(&mut xml).ok()?;
    id_in_custom_xml(&xml)
}

const CUSTOM_PART: &str = "docProps/custom.xml";
const CUSTOM_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.custom-properties+xml";
const CUSTOM_REL_TYPE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";
const CUSTOM_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties";
const VT_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes";
/// The format id every custom property carries (ECMA-376 Part 1 §22.3.2.1).
const CUSTOM_FMTID: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";

/// The id a presentation carries in its kept `docProps/custom.xml`, when it is a valid one.
pub(crate) fn presentation_document_id(doc: &deckcraft_model::Presentation) -> Option<String> {
    doc.kept_parts.iter().find(|k| k.name.eq_ignore_ascii_case(CUSTOM_PART)).and_then(|k| id_in_custom_xml(&k.xml))
}

/// `xml` (a `docProps/custom.xml`, or none) with the document id property set to `id`: an
/// existing property is replaced, the others are left as they are, and a new one gets the next
/// free `pid`.
pub(crate) fn custom_xml_with_id(xml: Option<&str>, id: &str) -> String {
    let property = |pid: u32| {
        format!("<property fmtid=\"{CUSTOM_FMTID}\" pid=\"{pid}\" name=\"{DOC_ID_PROP}\"><vt:lpwstr xmlns:vt=\"{VT_NS}\">{id}</vt:lpwstr></property>")
    };
    let Some(xml) = xml.filter(|x| x.contains("<Properties") || x.contains(":Properties")) else {
        return format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>
<Properties xmlns=\"{CUSTOM_NS}\" xmlns:vt=\"{VT_NS}\">{}</Properties>",
            property(2)
        );
    };
    // The existing property: from the `<property` that names it to its `</property>`.
    if let Some(name_at) = xml.find(&format!("name=\"{DOC_ID_PROP}\"")).or_else(|| xml.find(&format!("name='{DOC_ID_PROP}'")))
        && let Some(start) = xml.get(..name_at).and_then(|s| s.rfind("<property"))
        && let Some(end) = xml.get(name_at..).and_then(|s| s.find("</property>")).map(|e| name_at + e + "</property>".len())
    {
        let pid = xml.get(start..name_at).and_then(pid_of).unwrap_or(2);
        return format!("{}{}{}", &xml[..start], property(pid), &xml[end..]);
    }
    let mut pid = 1;
    let mut rest = xml;
    while let Some(at) = rest.find("pid=\"") {
        let tail = &rest[at + 5..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        pid = pid.max(digits.parse::<u32>().unwrap_or(0));
        rest = &tail[digits.len()..];
    }
    let new = property(pid.saturating_add(1).max(2));
    if let Some(close) = xml.rfind("</Properties>") {
        return format!("{}{new}{}", &xml[..close], &xml[close..]);
    }
    // A self-closing root (`<Properties .../>`): open it.
    match xml.rfind("/>") {
        Some(at) if xml.get(..at).is_some_and(|head| head.contains("<Properties")) => format!("{}>{new}</Properties>{}", &xml[..at], &xml[at + 2..]),
        _ => format!("{xml}{new}"),
    }
}

fn pid_of(attrs: &str) -> Option<u32> {
    let at = attrs.find("pid=\"")? + 5;
    attrs.get(at..)?.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

/// Write `id` into the presentation's custom properties (creating `docProps/custom.xml` as a
/// kept part when it has none), so a save carries it in the package.
pub(crate) fn set_presentation_document_id(doc: &mut deckcraft_model::Presentation, id: &str) {
    if let Some(part) = doc.kept_parts.iter_mut().find(|k| k.name.eq_ignore_ascii_case(CUSTOM_PART)) {
        part.xml = custom_xml_with_id(Some(&part.xml), id);
        return;
    }
    doc.kept_parts.push(deckcraft_model::KeptPart {
        name: CUSTOM_PART.to_owned(),
        content_type: CUSTOM_CONTENT_TYPE.to_owned(),
        xml: custom_xml_with_id(None, id),
        rel_type: CUSTOM_REL_TYPE.to_owned(),
        from_root: true,
    });
}

/// The document id of the presentation package at `path` (only its directory and that part are read).
pub(crate) fn file_document_id(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    package_document_id(std::io::BufReader::new(file))
}

/// This installation's random id: made once and kept in the settings folder. It says which
/// computer saved a version without saying anything about the computer.
pub(crate) fn installation_id(settings: &Path) -> String {
    let path = settings.join(INSTALLATION_FILE);
    if let Ok(meta) = std::fs::symlink_metadata(&path)
        && meta.is_file()
        && meta.len() <= 64
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        let id = text.trim();
        if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return id.to_ascii_lowercase();
        }
    }
    let id = random_bytes::<16>().map(|b| hex(&b)).unwrap_or_default();
    // Not kept (a read-only settings folder): versions of this run still share one id.
    let _ = super::write_private(&path, id.as_bytes());
    id
}

/// The operating system's version number, looked up once per run (never its name for the
/// machine).
pub(crate) fn os_version() -> String {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(read_os_version).clone()
}

fn version_digits(text: &str) -> String {
    let start = text.find(|c: char| c.is_ascii_digit()).unwrap_or(text.len());
    text.get(start..).unwrap_or("").chars().take_while(|c| c.is_ascii_digit() || *c == '.').take(32).collect()
}

fn read_os_version() -> String {
    #[cfg(windows)]
    {
        // `ver` prints "Microsoft Windows [Version 10.0.26100.1]".
        let output = std::process::Command::new("cmd").args(["/d", "/c", "ver"]).output();
        output
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|t| t.rsplit_once(' ').map(|(_, v)| version_digits(v)).unwrap_or_default())
            .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sw_vers").arg("-productVersion").output();
        output.ok().and_then(|o| String::from_utf8(o.stdout).ok()).map(|t| version_digits(&t)).unwrap_or_default()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        text.lines().find_map(|line| line.strip_prefix("VERSION_ID=")).map(|v| version_digits(v.trim_matches('"'))).unwrap_or_default()
    }
}

/// The account name, only so it can be kept out of what is recorded.
fn account_name() -> Option<String> {
    ["USERNAME", "USER", "LOGNAME"].iter().find_map(|k| std::env::var(k).ok()).filter(|n| !n.trim().is_empty())
}

fn home_dir() -> Option<PathBuf> {
    ["USERPROFILE", "HOME"].iter().find_map(std::env::var_os).filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// `path` without Windows' `\\?\` prefix.
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    text.strip_prefix(r"\\?\").map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// `path` below `base`, compared without case on Windows (where file names ignore it).
fn below(path: &Path, base: &Path) -> Option<PathBuf> {
    let (path, base) = (plain(path), plain(base));
    if let Ok(rest) = path.strip_prefix(&base) {
        return Some(rest.to_path_buf());
    }
    if cfg!(windows) {
        let mut parts = path.components();
        for want in base.components() {
            let got = parts.next()?;
            if got.as_os_str().to_string_lossy().to_lowercase() != want.as_os_str().to_string_lossy().to_lowercase() {
                return None;
            }
        }
        return Some(parts.as_path().to_path_buf());
    }
    None
}

fn slashed(rest: &Path) -> String {
    rest.components()
        .filter_map(|c| if let Component::Normal(n) = c { Some(n.to_string_lossy().into_owned()) } else { None })
        .collect::<Vec<_>>()
        .join("/")
}

/// The provider a document is kept with: `local`, `icloud`, `onedrive` or `gdrive`.
pub(crate) fn provider_of(path: &Path, roots: &[deckcraft_cloud::CloudRoot]) -> &'static str {
    deckcraft_cloud::locate(path, roots).map_or("local", |l| l.provider.id())
}

/// The path a version records: below the provider's folder (`OneDrive:/Reports/x.pptx`) or the
/// user's folder (`~/Documents/x.pptx`), and any folder named like the account replaced by
/// `<user>`, so the account name is never kept.
pub(crate) fn portable_path(path: &Path, roots: &[deckcraft_cloud::CloudRoot]) -> String {
    let path = plain(path);
    let text = if let Some(location) = deckcraft_cloud::locate(&path, roots)
        && let Some(rest) = below(&path, &location.root)
    {
        format!("{}:/{}", location.provider.id(), slashed(&rest))
    } else if let Some(home) = home_dir()
        && let Some(rest) = below(&path, &home)
    {
        format!("~/{}", slashed(&rest))
    } else {
        path.to_string_lossy().replace('\\', "/")
    };
    match account_name() {
        Some(account) => text
            .split('/')
            .map(|part| if part.eq_ignore_ascii_case(&account) || part.eq_ignore_ascii_case(&format!("{account}.local")) { "<user>" } else { part })
            .collect::<Vec<_>>()
            .join("/"),
        None => text,
    }
}

/// A recorded path back on this computer, when it can be (not when a part was hidden).
pub(crate) fn resolve_portable_path(text: &str, roots: &[deckcraft_cloud::CloudRoot]) -> Option<PathBuf> {
    if text.split('/').any(|part| part == "<user>" || part == "..") {
        return None;
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir().map(|home| home.join(rest));
    }
    if let Some((provider, rest)) = text.split_once(":/")
        && let Some(provider) = deckcraft_cloud::Provider::from_id(provider)
    {
        return roots.iter().find(|r| r.provider == provider).map(|r| r.path.join(rest));
    }
    let path = PathBuf::from(text);
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_ids_are_random_uuids() {
        let a = new_document_id().unwrap();
        let b = new_document_id().unwrap();
        assert_ne!(a, b);
        assert!(document_id_ok(&a), "{a}");
        assert_eq!(a.as_bytes()[14], b'4', "version 4");
        assert!(matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'), "RFC variant");
        assert_eq!(id_bucket(&a).len(), 32);
        for bad in [
            "",
            "../../etc/passwd",
            "ZZZZZZZZ-ZZZZ-4ZZZ-8ZZZ-ZZZZZZZZZZZZ",
            "12345678-1234-4234-8234-1234567890AB",
            "12345678123441234823412345678901ab",
        ] {
            assert!(!document_id_ok(bad), "{bad}");
        }
    }

    const ID: &str = "0de9ade2-3a07-42f2-b88a-2d9974cda145";
    const OTHER: &str = "11111111-2222-4333-8444-555555555555";

    #[test]
    fn the_id_goes_into_custom_xml_whatever_it_held() {
        // No part yet.
        let fresh = custom_xml_with_id(None, ID);
        assert_eq!(id_in_custom_xml(&fresh).as_deref(), Some(ID));
        assert_eq!(fresh.matches("<property").count(), 1);
        // Other properties stay, the new one gets the next free pid.
        let existing = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="ContentTypeId"><vt:lpwstr>0x0101</vt:lpwstr></property><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="7" name="Other"><vt:i4>3</vt:i4></property></Properties>"#;
        let with = custom_xml_with_id(Some(existing), ID);
        assert_eq!(id_in_custom_xml(&with).as_deref(), Some(ID));
        assert!(with.contains("0x0101") && with.contains("name=\"Other\""), "{with}");
        assert!(with.contains("pid=\"8\" name=\"DeckCraftDocumentId\""), "{with}");
        // Setting it again replaces the value in place; nothing else moves.
        let again = custom_xml_with_id(Some(&with), OTHER);
        assert_eq!(id_in_custom_xml(&again).as_deref(), Some(OTHER));
        assert_eq!(again.matches("DeckCraftDocumentId").count(), 1);
        assert!(again.contains("0x0101") && again.contains("pid=\"8\""));
        // A self-closing root is opened, an unreadable part is replaced.
        let empty = r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"/>"#;
        assert_eq!(id_in_custom_xml(&custom_xml_with_id(Some(empty), ID)).as_deref(), Some(ID));
        assert_eq!(id_in_custom_xml(&custom_xml_with_id(Some("not xml"), ID)).as_deref(), Some(ID));
        // Ids come from files: anything but a lower-case UUID is ignored.
        let hostile = fresh.replace(ID, "../../etc/passwd");
        assert!(id_in_custom_xml(&hostile).is_none());
    }

    #[test]
    fn the_id_survives_an_export_and_an_import_and_keeps_other_custom_properties() {
        let mut s = crate::Session::new();
        s.execute("file.new", &serde_json::json!({})).unwrap();
        let mut doc = (*s.active().unwrap().doc).clone();
        assert!(presentation_document_id(&doc).is_none());
        set_presentation_document_id(&mut doc, ID);
        assert_eq!(presentation_document_id(&doc).as_deref(), Some(ID));
        let bytes = crate::cmd::file::save_bytes(&doc, "pptx").unwrap();
        assert_eq!(package_document_id(std::io::Cursor::new(&bytes)).as_deref(), Some(ID));
        let mut back = crate::cmd::file::open_presentation("x.pptx", &bytes).unwrap();
        assert_eq!(presentation_document_id(&back).as_deref(), Some(ID), "read back as a kept part");
        // A package that already had custom properties: they are kept beside the id.
        back.kept_parts.retain(|k| !k.name.ends_with("custom.xml"));
        back.kept_parts.push(deckcraft_model::KeptPart {
            name: "docProps/custom.xml".into(),
            content_type: CUSTOM_CONTENT_TYPE.into(),
            xml: format!(r#"<Properties xmlns="{CUSTOM_NS}" xmlns:vt="{VT_NS}"><property fmtid="{CUSTOM_FMTID}" pid="2" name="ContentTypeId"><vt:lpwstr>0x0101</vt:lpwstr></property></Properties>"#),
            rel_type: CUSTOM_REL_TYPE.into(),
            from_root: true,
        });
        set_presentation_document_id(&mut back, OTHER);
        let bytes = crate::cmd::file::save_bytes(&back, "pptx").unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut xml = String::new();
        archive.by_name("docProps/custom.xml").unwrap().read_to_string(&mut xml).unwrap();
        assert!(xml.contains("ContentTypeId") && xml.contains(OTHER), "{xml}");
        let mut root = String::new();
        archive.by_name("_rels/.rels").unwrap().read_to_string(&mut root).unwrap();
        assert_eq!(root.matches("custom-properties").count(), 1, "one relationship: {root}");
        assert_eq!(package_document_id(std::io::Cursor::new(&bytes)).as_deref(), Some(OTHER));
    }

    #[test]
    fn recorded_paths_leave_out_the_account() {
        let home = home_dir().unwrap();
        let file = home.join("Documents").join("Report.pptx");
        assert_eq!(portable_path(&file, &[]), "~/Documents/Report.pptx");
        assert_eq!(resolve_portable_path("~/Documents/Report.pptx", &[]), Some(home.join("Documents/Report.pptx")));
        let root = deckcraft_cloud::CloudRoot { provider: deckcraft_cloud::Provider::OneDrive, path: home.join("OneDrive"), account: None };
        let cloud = home.join("OneDrive").join("Team").join("Plan.pptx");
        assert_eq!(portable_path(&cloud, std::slice::from_ref(&root)), "onedrive:/Team/Plan.pptx");
        assert_eq!(
            resolve_portable_path("onedrive:/Team/Plan.pptx", std::slice::from_ref(&root)),
            Some(home.join("OneDrive").join("Team/Plan.pptx"))
        );
        if let Some(account) = account_name() {
            let elsewhere = PathBuf::from(if cfg!(windows) { r"D:\Shared" } else { "/srv/shared" }).join(&account).join("x.pptx");
            let text = portable_path(&elsewhere, &[]);
            assert!(!text.to_lowercase().contains(&account.to_lowercase()), "{text}");
            assert!(resolve_portable_path(&text, &[]).is_none());
        }
    }
}
