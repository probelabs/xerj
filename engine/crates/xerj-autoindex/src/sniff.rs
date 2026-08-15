//! Content-based format detection. NEVER trusts file extensions.
//!
//! Order: magic bytes → binary check → text heuristics
//! (json/jsonl → html/xml → logs → sql dump → csv → yaml → txt).

use anyhow::Result;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Jsonl,
    Json,
    Csv,
    Logs,
    Xml,
    Html,
    Yaml,
    TxtProse,
    TxtLines,
    Pdf,
    Docx,
    Sqlite,
    SqlDump,
    /// Source code — AST-parsed by the matching tree-sitter grammar.
    Code,
    /// Unity text-serialized asset (scene/prefab/.asset/.mat/.anim/…):
    /// a `%YAML` + `%TAG !u! tag:unity3d.com` multi-document stream.
    UnityYaml,
    /// Unity `.meta` sidecar: plain YAML opening with `fileFormatVersion:`
    /// and carrying the asset `guid` — the join key for everything Unity.
    UnityMeta,
    /// Biovision motion capture: a skeleton HIERARCHY header followed by a
    /// large numeric MOTION block. Indexed as ONE metadata record per file
    /// (joints, frame count, duration) — the motion numbers are never read.
    Bvh,
    /// User-designated existence-only file (`--stub <glob>`): ONE name-card
    /// record, contents never opened. For corpus-specific data blobs the
    /// owner wants referenceable but not parsed — never assigned by
    /// sniffing, only by the CLI flag.
    Stub,
    Binary,
}

impl Family {
    pub fn as_str(&self) -> &'static str {
        match self {
            Family::Jsonl => "jsonl",
            Family::Json => "json",
            Family::Csv => "csv",
            Family::Logs => "logs",
            Family::Xml => "xml",
            Family::Html => "html",
            Family::Yaml => "yaml",
            Family::TxtProse => "txt-prose",
            Family::TxtLines => "txt-lines",
            Family::Pdf => "pdf",
            Family::Docx => "docx",
            Family::Sqlite => "sqlite",
            Family::SqlDump => "sqldump",
            Family::Code => "code",
            Family::UnityYaml => "unity",
            Family::UnityMeta => "unity-meta",
            Family::Bvh => "bvh",
            Family::Stub => "stub",
            Family::Binary => "binary",
        }
    }
    /// Document-family formats produce one record per document/section.
    pub fn is_document(&self) -> bool {
        matches!(self, Family::Pdf | Family::Docx | Family::TxtProse)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CsvDialect {
    pub delim: u8,
    pub has_header: bool,
    pub decimal_comma: bool,
}

#[derive(Debug, Clone)]
pub struct Sniffed {
    pub family: Family,
    pub gzip: bool,
    /// e.g. "png", "zip", "elf", "unknown" — set when family == Binary.
    pub binary_kind: Option<String>,
    pub csv: Option<CsvDialect>,
    /// "utf-8" or "windows-1252 (lossy)"
    pub encoding: &'static str,
    /// File name of the LOGICAL source (`app.py`), even when the sniffed
    /// content lives elsewhere. Durable preparation reads content-addressed
    /// snapshot blobs (`blobs/00000000`), so an extractor that recovers a
    /// parameter from its content path silently loses the name — that is how
    /// #294 turned every code file into junk. Name-derived decisions after
    /// sniffing must use this, never the content path.
    pub logical_name: Option<std::path::PathBuf>,
}

fn read_prefix(path: &Path, gzip: bool, n: usize) -> Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(n.min(1 << 20));
    if gzip {
        let mut r = flate2::read::MultiGzDecoder::new(f).take(n as u64);
        r.read_to_end(&mut buf).ok(); // truncated gz prefix is fine for sniffing
    } else {
        let mut r = f.take(n as u64);
        r.read_to_end(&mut buf)?;
    }
    Ok(buf)
}

pub fn sniff(path: &Path) -> Result<Sniffed> {
    sniff_with_name(path, path)
}

/// Classify bytes from `content_path` while retaining the logical filename
/// signals (currently source-code extensions) from `logical_path`.
///
/// Durable preparation uses this to classify an immutable snapshot blob
/// without losing the original name merely because the blob itself is named
/// by an ordinal.
pub fn sniff_with_name(content_path: &Path, logical_path: &Path) -> Result<Sniffed> {
    let head = read_prefix(content_path, false, 8)?;
    let gzip = head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b;
    let prefix = read_prefix(content_path, gzip, 8192)?;
    let mut s = sniff_bytes(&prefix, content_path, logical_path, gzip)?;
    s.gzip = gzip;
    s.logical_name = logical_path.file_name().map(std::path::PathBuf::from);
    Ok(s)
}

fn sniff_bytes(
    prefix: &[u8],
    content_path: &Path,
    logical_path: &Path,
    gzip: bool,
) -> Result<Sniffed> {
    let mk = |family: Family| Sniffed {
        family,
        gzip: false,
        binary_kind: None,
        csv: None,
        encoding: "utf-8",
        logical_name: None,
    };
    if prefix.is_empty() {
        let mut s = mk(Family::Binary);
        s.binary_kind = Some("empty".into());
        return Ok(s);
    }

    // 1. Magic bytes.
    if prefix.starts_with(b"%PDF-") {
        return Ok(mk(Family::Pdf));
    }
    if prefix.starts_with(b"SQLite format 3\0") {
        return Ok(mk(Family::Sqlite));
    }
    if prefix.starts_with(b"PK\x03\x04") {
        // zip container: DOCX iff it holds word/document.xml
        if !gzip {
            if let Ok(f) = std::fs::File::open(content_path) {
                if let Ok(mut z) = zip::ZipArchive::new(f) {
                    let is_docx = (0..z.len()).any(|i| {
                        z.by_index_raw(i)
                            .map(|e| e.name() == "word/document.xml")
                            .unwrap_or(false)
                    });
                    if is_docx {
                        return Ok(mk(Family::Docx));
                    }
                }
            }
        }
        let mut s = mk(Family::Binary);
        s.binary_kind = Some("zip".into());
        return Ok(s);
    }
    // Compressed image/audio/model payloads routinely pass the NUL and
    // control-char heuristics below (windows-1252 decodes almost every byte
    // to something printable), and a multi-MB PSD misread as prose costs
    // ~100x its size in RAM through sectioning. Magic bytes are the reliable
    // signal.
    for (magic, kind) in [
        (&b"\x89PNG"[..], "png"),
        (&b"GIF8"[..], "gif"),
        (&b"\xff\xd8\xff"[..], "jpeg"),
        (&b"\x7fELF"[..], "elf"),
        (&b"BM"[..], "bmp"),
        (&b"\x00\x00\x01\x00"[..], "ico"),
        (&b"8BPS"[..], "psd"),
        (&b"II*\x00"[..], "tiff"),
        (&b"MM\x00*"[..], "tiff"),
        (&b"RIFF"[..], "riff"),
        (&b"OggS"[..], "ogg"),
        (&b"fLaC"[..], "flac"),
        (&b"ID3"[..], "mp3"),
        (&b"Kaydara FBX Binary"[..], "fbx"),
        (&b"\x76\x2f\x31\x01"[..], "exr"),
    ] {
        if prefix.starts_with(magic) {
            let mut s = mk(Family::Binary);
            s.binary_kind = Some(kind.into());
            return Ok(s);
        }
    }

    // 2. Binary vs text: decode UTF-8, fall back windows-1252.
    let (text, encoding) = decode(prefix);
    let nul = prefix.iter().filter(|&&b| b == 0).count();
    if nul * 10 > prefix.len() {
        let mut s = mk(Family::Binary);
        s.binary_kind = Some("unknown".into());
        return Ok(s);
    }
    // High ratio of control chars (excluding \t \n \r) → binary.
    let ctrl = text
        .chars()
        .filter(|c| (*c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r'))
        .count();
    if ctrl * 10 > text.chars().count().max(1) {
        let mut s = mk(Family::Binary);
        s.binary_kind = Some("unknown".into());
        return Ok(s);
    }
    // A prefix that only decoded via LOSSY windows-1252 and is majority
    // high-byte is pixel/float soup, not prose in a legacy encoding: real
    // windows-1252 text (accented European prose) runs well under 30%
    // non-ASCII, while raw image channels run ~50%. Without this, a large
    // TGA classified as txt-prose is amplified ~100x in RAM by sectioning.
    //
    // Gated on the windows-1252 fallback SPECIFICALLY, and skipped when the
    // bytes are plausibly legacy CJK: Shift-JIS/GBK/Big5/EUC-KR prose is
    // ~100% high-byte, so the unqualified test junked every legacy-encoded
    // CJK document in the corpus.
    let legacy_cjk = encoding == WINDOWS_1252_LOSSY && looks_like_legacy_cjk(prefix);
    if encoding == WINDOWS_1252_LOSSY && !legacy_cjk {
        let total = text.chars().count().max(1);
        let non_ascii = text.chars().filter(|c| !c.is_ascii()).count();
        if non_ascii * 10 > total * 3 {
            let mut s = mk(Family::Binary);
            s.binary_kind = Some("unknown".into());
            return Ok(s);
        }
    }

    // 2b. Unity text serialization, detected by content (never extension):
    // scenes/prefabs/assets open with `%YAML` and declare the Unity tag
    // namespace; `.meta` sidecars open with `fileFormatVersion:` and carry a
    // `guid:`. Binary-serialized Unity assets fail both checks and fall
    // through to the binary/text heuristics as before.
    {
        let body = text.trim_start_matches('\u{feff}');
        if body.starts_with("%YAML") && body.contains("%TAG !u! tag:unity3d.com") {
            let mut s = mk(Family::UnityYaml);
            s.encoding = encoding;
            return Ok(s);
        }
        let first_line = body.lines().next().unwrap_or("");
        if first_line.starts_with("fileFormatVersion:")
            && body.lines().any(|l| l.starts_with("guid:"))
        {
            let mut s = mk(Family::UnityMeta);
            s.encoding = encoding;
            return Ok(s);
        }
        // BVH motion capture: `HIERARCHY` opener with a `ROOT <name>` next.
        // Without this the numeric MOTION block classified as txt-lines and
        // indexed millions of meaningless number rows.
        if first_line.trim() == "HIERARCHY"
            && body
                .lines()
                .nth(1)
                .is_some_and(|l| l.trim_start().starts_with("ROOT "))
        {
            let mut s = mk(Family::Bvh);
            s.encoding = encoding;
            return Ok(s);
        }
    }

    // 2c. Source code: a known code extension whose content is text. We only
    // reach here after the binary guards above, so a text `.py`/`.rs`/`.go`/…
    // routes to the tree-sitter AST extractor (crate::extract::code). Extension
    // is the right signal — code vs prose is not reliably content-sniffable.
    if let Some(ext) = logical_path.extension().and_then(|e| e.to_str()) {
        if crate::extract::code::is_code_ext(ext) {
            let mut s = mk(Family::Code);
            s.encoding = encoding;
            return Ok(s);
        }
    }

    // 3. Text heuristics — complete lines only (last line may be truncated).
    let mut lines: Vec<&str> = text.lines().collect();
    if !text.ends_with('\n') && lines.len() > 1 {
        lines.pop();
    }
    let nonblank: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();

    let mut out = mk(classify_text(&text, &nonblank));
    out.encoding = encoding;
    if out.family == Family::Csv {
        out.csv = sniff_csv_dialect(&nonblank);
        if out.csv.is_none() {
            out.family = txt_kind(&nonblank);
        }
    }
    Ok(out)
}

/// The label for the last-resort decode. Every byte maps to *something* in
/// windows-1252, so this never fails and therefore carries no evidence that
/// the bytes are text — the binary heuristics key off this exact string.
pub const WINDOWS_1252_LOSSY: &str = "windows-1252 (lossy)";

/// Scripts written without spaces between words ("scriptio continua").
///
/// Whitespace density cannot be used to tell prose from byte soup for these:
/// Chinese, Japanese, Korean, Thai, Lao, Khmer and Burmese prose runs at
/// essentially 0% intra-line whitespace, the same as random bytes. Their
/// presence is instead POSITIVE evidence of text — neither pixel channels nor
/// float dumps produce ideographs.
///
/// The script set is the one Lucene singles out for exactly this reason:
/// `CJKBigramFilter` (`lucene/analysis/common/src/java/org/apache/lucene/
/// analysis/cjk/CJKBigramFilter.java:52-118`, Apache-2.0) defines `HAN`,
/// `HIRAGANA`, `KATAKANA` and `HANGUL` flags because a whitespace tokenizer
/// cannot segment them, and its default constructor
/// (`CJKBigramFilter.java:119-126`) enables all four together. Thai, Lao,
/// Khmer and Myanmar are added here on the same principle — Lucene reaches
/// for `ICUTokenizer`/dictionary breaking for those rather than a bigram
/// filter, but the property this function tests (no inter-word spaces) is
/// identical. Adapted, not copied: Lucene is deciding how to TOKENIZE known
/// text; this only decides whether the bytes are text at all.
fn is_scriptio_continua(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF   // Hangul Jamo
        | 0x0E00..=0x0EFF // Thai, Lao
        | 0x1000..=0x109F // Myanmar
        | 0x1780..=0x17FF // Khmer
        | 0x3000..=0x303F // CJK symbols and punctuation (、。「」)
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7AF // Hangul Syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFF00..=0xFFEF // Halfwidth and Fullwidth Forms
        | 0x20000..=0x2A6DF // CJK Unified Ideographs Extension B
    )
}

/// Share of `text` that is scriptio-continua, in percent.
fn continua_percent(text: &str) -> usize {
    let mut total = 0usize;
    let mut hits = 0usize;
    for c in text.chars() {
        total += 1;
        if is_scriptio_continua(c) {
            hits += 1;
        }
    }
    if total == 0 {
        return 0;
    }
    hits * 100 / total
}

/// Whether `bytes` are plausibly text in a legacy CJK encoding.
///
/// This is a TEXT-NESS test and nothing more. It deliberately does NOT return
/// which encoding: Shift-JIS, GBK, Big5 and EUC-KR share most of the same
/// valid double-byte space, so a GBK document also decodes losslessly as
/// Shift-JIS (and vice versa) with a high ideograph share either way. Telling
/// them apart needs a statistical language model — what `chardet`/ICU do —
/// which this crate does not have, and guessing would silently replace a
/// Chinese document with plausible-looking Japanese mojibake. Tracked as a
/// follow-up; see CHANGELOG.
///
/// What it IS sound for is the only question asked here: are these bytes a
/// pixel/float payload, or somebody's prose? The pixel-soup guards below key
/// off high-byte density and whitespace density, and legacy CJK prose looks
/// exactly like soup under both — ~100% high-byte and ~0% whitespace — so
/// without this they junk the file outright. Requiring a LOSSLESS decode
/// (one undecodable pair disqualifies it) plus a substantially ideographic
/// result keeps European windows-1252 prose out: its high bytes sit isolated
/// among ASCII, so most form no valid double-byte pair at all.
fn looks_like_legacy_cjk(bytes: &[u8]) -> bool {
    [
        encoding_rs::SHIFT_JIS,
        encoding_rs::GBK,
        encoding_rs::BIG5,
        encoding_rs::EUC_KR,
    ]
    .into_iter()
    .any(|enc| {
        let (s, _, had_errors) = enc.decode(bytes);
        !had_errors && continua_percent(&s) >= 30
    })
}

fn decode(bytes: &[u8]) -> (String, &'static str) {
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), "utf-8"),
        Err(e) => {
            // Tolerate a multi-byte char cut at the prefix boundary.
            if e.valid_up_to() + 4 >= bytes.len() {
                (
                    String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
                    "utf-8",
                )
            } else {
                let (s, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
                (s.into_owned(), WINDOWS_1252_LOSSY)
            }
        }
    }
}

/// Decode a whole byte buffer for extraction (same policy as sniffing).
pub fn decode_text(bytes: &[u8]) -> (String, &'static str) {
    decode(bytes)
}

fn classify_text(text: &str, nonblank: &[&str]) -> Family {
    let trimmed = text.trim_start();
    // A lone `[section]` opening line that does not parse as JSON is a
    // TOML/INI table header (`[package]` in Cargo.toml, `[Unit]` in a
    // systemd unit), not a JSON array. Without this guard every such file
    // fell into the JSON branch below (any '['-opener passes
    // `looks_like_json_start`), reached the JSON extractor, and was junked
    // — which, for Cargo.toml, also silently disabled the cratecite
    // detector on every real repository (its crate table only holds
    // indexed files). Live-verified 2026-07-30: three Cargo.tomls junked
    // as "json candidate family" before this guard, indexed after.
    let ini_table_header = nonblank.first().is_some_and(|l| {
        let t = l.trim();
        t.starts_with('[')
            && t.ends_with(']')
            && serde_json::from_str::<serde_json::Value>(t).is_err()
    });
    // JSON / JSONL
    if !ini_table_header && (trimmed.starts_with('{') || trimmed.starts_with('[')) {
        if nonblank.len() >= 2 {
            let parse_ok = nonblank
                .iter()
                .filter(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
                .count();
            if parse_ok * 10 >= nonblank.len() * 9 {
                return Family::Jsonl;
            }
        } else if nonblank.len() == 1
            && serde_json::from_str::<serde_json::Value>(nonblank[0]).is_ok()
        {
            // single complete JSON line — treat as JSON value file
            return Family::Json;
        }
        // Pretty-printed or multi-line JSON value.
        if looks_like_json_start(trimmed) {
            return Family::Json;
        }
    }

    // HTML / XML — declaration within the first 256 bytes.
    let head_lc: String = text.chars().take(256).collect::<String>().to_lowercase();
    if head_lc.contains("<!doctype html") || head_lc.contains("<html") {
        return Family::Html;
    }
    if head_lc.contains("<?xml") || (trimmed.starts_with('<') && text.contains("</")) {
        // xhtml disguised as xml?
        let lc: String = text.to_lowercase();
        if lc.contains("<html") || lc.contains("<body") {
            return Family::Html;
        }
        return Family::Xml;
    }

    // Log lines
    if nonblank.len() >= 3 {
        let hits = nonblank
            .iter()
            .filter(|l| crate::extract::logs::probe_line(l))
            .count();
        if hits * 10 >= nonblank.len() * 6 {
            return Family::Logs;
        }
    }

    // SQL dump
    let upper: String = text.chars().take(4096).collect::<String>().to_uppercase();
    if (upper.contains("CREATE TABLE") || upper.contains("INSERT INTO")) && text.contains(';') {
        return Family::SqlDump;
    }

    // CSV — dialect probe happens in caller; here just a cheap plausibility test.
    if nonblank.len() >= 2 && sniff_csv_dialect(nonblank).is_some() {
        return Family::Csv;
    }

    // YAML
    if nonblank.first().map(|l| l.trim() == "---").unwrap_or(false)
        || yaml_line_ratio(nonblank) >= 0.6
    {
        return Family::Yaml;
    }

    txt_kind(nonblank)
}

fn looks_like_json_start(t: &str) -> bool {
    // starts with { or [ and the first ~200 chars look like JSON tokens
    let head: String = t.chars().take(200).collect();
    head.contains(':') || head.contains('[') || head.contains('{')
}

fn yaml_line_ratio(nonblank: &[&str]) -> f64 {
    if nonblank.len() < 3 {
        return 0.0;
    }
    let re = regex::Regex::new(r"^\s*(- )?[\w.@/-]+:(\s|$)").unwrap();
    let hits = nonblank
        .iter()
        .filter(|l| {
            let t = l.trim_start();
            // Markdown task-list items (`- [ ]` / `- [x]`) are NOT YAML
            // evidence: `- [ ] text` is invalid YAML (flow sequence followed
            // by a scalar), while checklists are everywhere in real notes.
            // Counting them here routed whole checklist files into the YAML
            // extractor, which can only junk-file them.
            let checkbox =
                t.starts_with("- [ ] ") || t.starts_with("- [x] ") || t.starts_with("- [X] ");
            re.is_match(l) || (t.starts_with("- ") && !checkbox)
        })
        .count();
    hits as f64 / nonblank.len() as f64
}

fn txt_kind(nonblank: &[&str]) -> Family {
    if nonblank.is_empty() {
        return Family::TxtLines;
    }
    // NOTE — a whitespace-density guard ("text over 4 KiB with under 5%
    // whitespace is pixel soup, junk it") was proposed here to catch raw TGA
    // and `.bytes` payloads that decode into printable characters. It is not
    // present, deliberately, because low whitespace density does not mean
    // "not text":
    //
    //   * Chinese, Japanese, Korean, Thai, Lao, Khmer and Burmese prose is
    //     scriptio continua — `nonblank` comes from `text.lines()`, so the
    //     newlines are already stripped and only INTRA-LINE whitespace counts,
    //     which those scripts do not have. Every such document over ~4 KiB
    //     would be junked, in every corpus, worldwide.
    //   * base64 blobs, hex dumps, FASTA/genomic sequences, single-line
    //     minified payloads and long-token files are all legitimate text with
    //     near-zero whitespace.
    //   * `failure_resume_http_tests::legacy_key_collision_fails_before_
    //     visibility_with_scoped_guidance` builds a 65,537-byte fixture of one
    //     repeated ASCII letter. With the guard in place that file sniffs as
    //     binary and the run exits 3 instead of 0 — the false positive is
    //     reachable from this repo's own suite.
    //
    // High-byte payloads (which is what a real TGA is) are still caught, by
    // the windows-1252 non-ASCII test in `sniff_bytes`. Bounding sectioning
    // memory is the right fix for the RAM concern; silently deleting files
    // that look unusual is not. Tracked as a follow-up.
    let avg_len = nonblank.iter().map(|l| l.len()).sum::<usize>() as f64 / nonblank.len() as f64;
    if avg_len > 60.0 {
        return Family::TxtProse;
    }
    // A handful of short lines in a note-like file is still prose.
    if nonblank.len() <= 5 {
        return Family::TxtProse;
    }

    // Line LENGTH alone splits documents of the same kind.  A markdown
    // postmortem with `## Headings` averages ~50 chars over 7 lines and used
    // to land in TxtLines, while a 5-line runbook averaging 59 chars landed in
    // TxtProse — same content type, two different families, therefore two
    // different datasets with two different field names (`text` vs `body`).
    // Cross-index BM25 statistics are then incomparable and a caller has to
    // query both fields.
    //
    // Sentence density is the property that actually distinguishes a document
    // from a record stream: prose lines end in terminal punctuation, whereas
    // log lines, CSV rows and source code do not.  Measured on a mixed corpus:
    // markdown 0.43-0.57, nginx access logs 0.00, syslog 0.20, Rust/Python/JS
    // source 0.00-0.10.
    let sentences = nonblank
        .iter()
        .filter(|l| {
            let t = l.trim_end();
            t.ends_with('.') || t.ends_with('!') || t.ends_with('?')
        })
        .count();
    let sentence_ratio = sentences as f64 / nonblank.len() as f64;
    if sentence_ratio >= 0.40 {
        return Family::TxtProse;
    }

    // Hard-wrapped markdown rescue. Sentence density per LINE undercounts
    // prose whose author wraps at ~70-80 columns: sentences end mid-line, so
    // a five-paragraph note with a `# Title` scores ~0.30 and landed in
    // TxtLines — which silently cost it its title, its section anchors, and
    // (second brain) its wikilink detection. Content evidence, not the
    // extension: a file that OPENS with an ATX heading and shows either
    // markdown link syntax or some terminal punctuation is a markdown
    // document. The heading-opener requirement keeps shebang'd scripts
    // (`#!…`) and most code/config out; the second signal keeps out comment
    // banners over pure record streams.
    let md_link = nonblank
        .iter()
        .any(|l| l.contains("[[") || l.contains("]("));
    if md_heading(nonblank[0]) && (md_link || sentence_ratio >= 0.20) {
        return Family::TxtProse;
    }
    Family::TxtLines
}

/// `# Title` … `###### Title` — an ATX markdown heading (1-6 `#`, a space,
/// then text). `#!/bin/sh` and bare `#` fail the space-then-text rule.
fn md_heading(line: &str) -> bool {
    let t = line.trim_start();
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && t.as_bytes().get(hashes) == Some(&b' ') && t.len() > hashes + 1
}

/// Quote-aware field split (supports " and ' quoting).
fn split_quoted(line: &str, delim: u8) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c as u32 == delim as u32 {
                    fields.push(std::mem::take(&mut cur));
                } else {
                    cur.push(c);
                }
            }
        }
    }
    fields.push(cur);
    fields
}

fn sniff_csv_dialect(nonblank: &[&str]) -> Option<CsvDialect> {
    if nonblank.len() < 2 {
        return None;
    }
    let sample: Vec<&str> = nonblank.iter().take(64).copied().collect();
    let mut best: Option<(u8, usize)> = None; // (delim, field count)
    for delim in *b",;\t|" {
        let counts: Vec<usize> = sample
            .iter()
            .map(|l| split_quoted(l, delim).len())
            .collect();
        let first = counts[0];
        if first < 2 {
            continue;
        }
        let consistent = counts.iter().filter(|&&c| c == first).count();
        // ≥90% of lines share the same field count
        if consistent * 10 >= counts.len() * 9 {
            match best {
                Some((_, bc)) if bc >= first => {}
                _ => best = Some((delim, first)),
            }
        }
    }
    let (delim, _) = best?;
    let head_fields = split_quoted(sample[0], delim);
    let numericish = |s: &str| {
        let t = s.trim();
        !t.is_empty()
            && t.chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+'))
            && t.chars().any(|c| c.is_ascii_digit())
    };
    let has_header = {
        let mut distinct = std::collections::HashSet::new();
        let all_nonnum = head_fields.iter().all(|f| !numericish(f));
        let all_distinct = head_fields
            .iter()
            .all(|f| distinct.insert(f.trim().to_string()));
        let body_has_num = sample
            .iter()
            .skip(1)
            .any(|l| split_quoted(l, delim).iter().any(|f| numericish(f)));
        all_nonnum && all_distinct && body_has_num
    };
    // decimal comma: with ';' delimiter, a meaningful share of fields look like 12,3
    let decimal_comma = if delim == b';' {
        let re = regex::Regex::new(r"^-?\d{1,9},\d+$").unwrap();
        let (mut num, mut hits) = (0usize, 0usize);
        for l in sample.iter().skip(if has_header { 1 } else { 0 }) {
            for f in split_quoted(l, delim) {
                let t = f.trim().to_string();
                if numericish(&t) {
                    num += 1;
                    if re.is_match(&t) {
                        hits += 1;
                    }
                }
            }
        }
        num > 0 && hits * 10 >= num * 3
    } else {
        false
    };
    Some(CsvDialect {
        delim,
        has_header,
        decimal_comma,
    })
}

#[cfg(test)]
mod unity_sniff_tests {
    use super::*;
    use std::path::Path;

    fn sniff_str(s: &str, name: &str) -> Family {
        sniff_bytes(s.as_bytes(), Path::new(name), Path::new(name), false)
            .unwrap()
            .family
    }

    #[test]
    fn unity_tagged_yaml_is_detected_by_header_not_extension() {
        let scene =
            "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &123\nGameObject:\n  m_Name: X\n";
        assert_eq!(sniff_str(scene, "Main.unity"), Family::UnityYaml);
        assert_eq!(
            sniff_str(scene, "renamed.txt"),
            Family::UnityYaml,
            "content decides, never the extension"
        );
    }

    #[test]
    fn a_bom_before_the_yaml_directive_is_tolerated() {
        let scene =
            "\u{feff}%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!1 &1\nGameObject:\n  m_Name: X\n";
        assert_eq!(sniff_str(scene, "Main.unity"), Family::UnityYaml);
    }

    #[test]
    fn meta_needs_the_first_line_rule_and_a_guid() {
        let meta =
            "fileFormatVersion: 2\nguid: 9f1c4d0ab2e34f6\nMonoImporter:\n  serializedVersion: 2\n";
        assert_eq!(sniff_str(meta, "Player.cs.meta"), Family::UnityMeta);
        let stray = "config: true\nfileFormatVersion: 2\nguid: abc\n";
        assert_ne!(
            sniff_str(stray, "some.yaml"),
            Family::UnityMeta,
            "guid keys inside ordinary YAML must not reclassify it"
        );
        let no_guid = "fileFormatVersion: 2\nsettings:\n  a: 1\n";
        assert_ne!(sniff_str(no_guid, "x.meta"), Family::UnityMeta);
    }

    #[test]
    fn bvh_is_detected_by_hierarchy_root_header() {
        let bvh = "HIERARCHY\nROOT Hips\n{\n  OFFSET 0 90 0\n  CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation\n}\nMOTION\nFrames: 2\nFrame Time: 0.033\n1 2 3\n4 5 6\n";
        assert_eq!(sniff_str(bvh, "clip.bvh"), Family::Bvh);
        assert_eq!(
            sniff_str(bvh, "clip.txt"),
            Family::Bvh,
            "content decides, never the extension"
        );
        let not_bvh = "HIERARCHY\nof needs (Maslow):\n- physiological\n- safety\n";
        assert_ne!(sniff_str(not_bvh, "notes.txt"), Family::Bvh);
    }

    #[test]
    fn plain_yaml_without_the_unity_tag_stays_yaml() {
        let plain = "%YAML 1.2\n---\nkey: value\nother: 1\nnested:\n  a: 2\n";
        assert_ne!(sniff_str(plain, "doc.yaml"), Family::UnityYaml);
    }

    /// Regression: PSD image data decoded via lossy windows-1252 passed the
    /// NUL/control-char heuristics and classified as txt-prose, turning a
    /// multi-MB texture into ~100x its size of prose sections. Media magic
    /// bytes must win before any text heuristic runs.
    #[test]
    fn media_containers_are_binary_by_magic_not_heuristics() {
        for (name, head) in [
            ("t.psd", &b"8BPS\x00\x01"[..]),
            ("t.tif", &b"II*\x00\x08\x00"[..]),
            ("t.tif2", &b"MM\x00*\x00\x08"[..]),
            ("t.wav", &b"RIFF\x24\x08\x00\x00WAVE"[..]),
            ("t.ogg", &b"OggS\x00\x02"[..]),
            ("t.fbx", &b"Kaydara FBX Binary  \x00"[..]),
        ] {
            let mut bytes = head.to_vec();
            // A printable tail that WOULD pass the prose heuristics.
            bytes.extend(b"lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(20));
            let sn = sniff_bytes(&bytes, Path::new(name), Path::new(name), false).unwrap();
            assert_eq!(sn.family, Family::Binary, "{name} must be binary");
        }
    }

    /// Whitespace density must NOT be used to junk text. This pins the
    /// counter-examples that make the rejected guard unsafe: a long run of one
    /// ASCII letter (the shape of this repo's own 65,537-byte resume-key
    /// fixture), and a base64 blob. Both are >4 KiB with zero whitespace, and
    /// both are text.
    #[test]
    fn whitespace_free_ascii_text_is_not_junked_as_binary() {
        let mut repeated = "x".repeat(65_536);
        repeated.push('b');
        let b64: String = std::iter::repeat_n("QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo", 300)
            .collect::<Vec<_>>()
            .join("");
        for (label, text) in [("repeated-letter", &repeated), ("base64", &b64)] {
            assert!(text.chars().filter(|c| c.is_whitespace()).count() == 0);
            let sn = sniff_bytes(
                text.as_bytes(),
                Path::new("blob.txt"),
                Path::new("blob.txt"),
                false,
            )
            .unwrap();
            assert_ne!(
                sn.family,
                Family::Binary,
                "{label}: whitespace-free ASCII is still text"
            );
        }
        // Real prose is unaffected either way.
        let prose = "The quick brown fox jumps over the lazy dog. ".repeat(200);
        let sn = sniff_bytes(
            prose.as_bytes(),
            Path::new("note.txt"),
            Path::new("note.txt"),
            false,
        )
        .unwrap();
        assert_ne!(sn.family, Family::Binary, "real prose must stay text");
    }

    /// Regression: a whitespace-density guard judged every script by a rule
    /// that only holds for space-delimited ones. `nonblank` is built from
    /// `text.lines()`, so newlines are already gone and only INTRA-LINE
    /// whitespace counts — which CJK/Thai/Lao/Khmer/Burmese prose does not
    /// have. Every such document over ~4 KB became `Family::Binary` and was
    /// junked as "binary content (unknown)", in a Unity PR, for users with no
    /// Unity project. Both original tests used Latin prose and ASCII soup, so
    /// neither could see it. The guard is gone; this pins the outcome so it
    /// cannot come back in another form.
    #[test]
    fn scriptio_continua_prose_is_not_mistaken_for_binary_soup() {
        // Each sample is >= 4096 chars (the old guard's gate) and has NO
        // spaces, exactly like the real documents that were being junked.
        for (label, unit) in [
            ("chinese", "本文档描述了系统的架构设计与实现细节。"),
            (
                "japanese",
                "この文書はシステムの設計と実装について説明します。",
            ),
            ("korean", "이문서는시스템설계와구현에대해설명합니다"),
            ("thai", "เอกสารนี้อธิบายการออกแบบและการใช้งานของระบบ"),
        ] {
            let text = unit.repeat(4096 / unit.chars().count() + 2);
            assert!(
                text.chars().count() >= 4096,
                "{label}: sample must clear the 4096-char gate"
            );
            let ws = text.chars().filter(|c| c.is_whitespace()).count();
            assert!(
                ws * 20 < text.chars().count(),
                "{label}: sample must be below the 5% whitespace ratio, else \
                 it would have passed the old guard for the wrong reason"
            );
            let sn = sniff_bytes(
                text.as_bytes(),
                Path::new("notes.txt"),
                Path::new("notes.txt"),
                false,
            )
            .unwrap();
            assert_ne!(
                sn.family,
                Family::Binary,
                "{label}: scriptio-continua prose must not be junked as binary"
            );
        }
    }

    /// Regression: the sibling guard keyed off `encoding != "utf-8"`, and a
    /// legacy CJK encoding decodes as lossy windows-1252 into ~100%
    /// high-byte mojibake — so every Shift-JIS/GBK/Big5/EUC-KR document was
    /// junked before any family heuristic ran.
    ///
    /// The assertion is deliberately only "not binary". This crate cannot
    /// tell these encodings apart (they share most of their valid
    /// double-byte space) and does not pretend to: the file is still decoded
    /// as windows-1252, exactly as it was before this branch, which is
    /// mojibake — but mojibake that is INDEXED rather than deleted.
    #[test]
    fn legacy_encoded_cjk_prose_is_not_junked_as_binary() {
        for (label, enc, unit) in [
            (
                "shift_jis",
                encoding_rs::SHIFT_JIS,
                "この文書はシステムの設計について説明します。",
            ),
            ("gbk", encoding_rs::GBK, "本文档描述了系统的架构设计。"),
            ("big5", encoding_rs::BIG5, "本文件描述系統的架構設計。"),
            (
                "euc-kr",
                encoding_rs::EUC_KR,
                "이문서는시스템설계를설명합니다",
            ),
        ] {
            let text = unit.repeat(4096 / unit.chars().count() + 2);
            let (bytes, _, had_errors) = enc.encode(&text);
            assert!(!had_errors, "{label}: fixture must encode cleanly");
            assert!(
                std::str::from_utf8(&bytes).is_err(),
                "{label}: fixture must not be valid UTF-8, or it proves nothing"
            );
            assert!(
                looks_like_legacy_cjk(&bytes),
                "{label}: text-ness probe must recognise this as prose"
            );
            let sn =
                sniff_bytes(&bytes, Path::new("doc.txt"), Path::new("doc.txt"), false).unwrap();
            assert_ne!(sn.family, Family::Binary, "{label}: must not be junked");
        }
    }

    /// The legacy-CJK probe must not capture European text — that would
    /// replace real windows-1252 prose with plausible-looking ideographs.
    #[test]
    fn european_windows_1252_prose_is_not_claimed_as_cjk() {
        let text = "Le système décrit ici gère les données réservées à \
                    l'opérateur, prêtes à être exportées. ";
        let long = text.repeat(80);
        let (bytes, _, _) = encoding_rs::WINDOWS_1252.encode(&long);
        assert!(std::str::from_utf8(&bytes).is_err());
        let (_, enc) = decode(&bytes);
        assert_eq!(enc, WINDOWS_1252_LOSSY, "must stay windows-1252");
        assert!(
            !looks_like_legacy_cjk(&bytes),
            "accented Latin must not be claimed as CJK"
        );
        let sn = sniff_bytes(&bytes, Path::new("note.txt"), Path::new("note.txt"), false).unwrap();
        assert_ne!(sn.family, Family::Binary, "accented prose is still prose");
    }

    /// The pixel-soup protection the guards exist for must survive the fix:
    /// high-byte float/pixel payloads still have to be caught.
    #[test]
    fn high_byte_pixel_soup_is_still_binary() {
        // Bytes that are not valid UTF-8, not valid in any legacy CJK
        // encoding as a whole, and majority high-byte — a raw texture.
        let soup: Vec<u8> = (0..8192u32)
            .map(|i| (0x80 + (i * 37) % 0x7f) as u8)
            .collect();
        assert!(std::str::from_utf8(&soup).is_err());
        let sn = sniff_bytes(
            &soup,
            Path::new("texture.tga"),
            Path::new("texture.tga"),
            false,
        )
        .unwrap();
        assert_eq!(sn.family, Family::Binary, "pixel soup must stay binary");
    }

    #[test]
    fn binary_serialized_unity_assets_stay_binary() {
        let mut bytes = b"UnityFS\x00\x00\x00\x00\x08".to_vec();
        bytes.extend(std::iter::repeat_n(0u8, 64));
        let sn = sniff_bytes(
            &bytes,
            Path::new("scene.unity"),
            Path::new("scene.unity"),
            false,
        )
        .unwrap();
        assert_eq!(sn.family, Family::Binary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(s: &str) -> Family {
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        classify_text(s, &lines)
    }

    #[test]
    fn sniff_families() {
        assert_eq!(classify("{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n"), Family::Jsonl);
        assert_eq!(
            classify("{\n  \"a\": 1,\n  \"b\": [1,2]\n}\n"),
            Family::Json
        );
        assert_eq!(
            classify("<!DOCTYPE html>\n<html><head></head></html>"),
            Family::Html
        );
        assert_eq!(
            classify("<?xml version='1.0'?>\n<r><a>1</a></r>"),
            Family::Xml
        );
        assert_eq!(classify("a,b,c\n1,2,3\n4,5,6\n"), Family::Csv);
        assert_eq!(
            classify("CREATE TABLE `t` (\n `a` int\n);\nINSERT INTO `t` VALUES (1);\n"),
            Family::SqlDump
        );
        assert_eq!(
            classify("key: value\nother: 1\nnested:\n  a: 2\n"),
            Family::Yaml
        );
    }

    /// `[package]`-style openers are TOML/INI table headers, not JSON
    /// arrays. Before the guard, Cargo.toml sniffed as Json, the JSON
    /// extractor junked it, and cratecite's crate table stayed empty on
    /// every real repository (its dst must be an INDEXED Cargo.toml).
    #[test]
    fn toml_table_header_is_not_json() {
        let cargo = "[package]\nname = \"xerj-fts\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
                     [dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\n\
                     serde_json = \"1\"\nanyhow = \"1\"\n";
        assert_eq!(classify(cargo), Family::TxtLines);
        let ini = "[Unit]\nDescription=demo\nAfter=network.target\n\n\
                   [Service]\nExecStart=/bin/true\nRestart=always\n\n\
                   [Install]\nWantedBy=multi-user.target\n";
        assert_eq!(classify(ini), Family::TxtLines);
        // …while real JSON arrays keep their family.
        assert_eq!(classify("[1, 2, 3]\n"), Family::Json);
        assert_eq!(
            classify("[\n  {\"a\": 1},\n  {\"a\": 2}\n]\n"),
            Family::Json
        );
    }

    #[test]
    fn csv_dialect_semicolon_decimal_comma() {
        let lines = vec![
            "geraet;zeitpunkt;temperatur_c",
            "dev-1;2026-03-09T02:09:26Z;50,6",
            "dev-2;2026-03-10T19:10:36Z;57,0",
        ];
        let d = sniff_csv_dialect(&lines).unwrap();
        assert_eq!(d.delim, b';');
        assert!(d.has_header);
        assert!(d.decimal_comma);
    }
}

#[cfg(test)]
mod text_family_tests {
    use super::*;

    fn kind(text: &str) -> Family {
        let nonblank: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        txt_kind(&nonblank)
    }

    /// Full text classifier — access logs and syslog are claimed by the `Logs`
    /// family before `txt_kind` is ever consulted, so they must be asserted
    /// through the real entry point rather than against `txt_kind` directly.
    fn classify_full(text: &str) -> Family {
        let nonblank: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        classify_text(text, &nonblank)
    }

    /// Regression: a markdown document with `## Headings` averages ~50 chars
    /// over 7 lines, which the length-only rule classified as TxtLines — the
    /// same corpus's 5-line runbook (avg 59) went to TxtProse. Same content
    /// type, two families, two field names, incomparable BM25 statistics.
    #[test]
    fn markdown_with_headings_is_prose() {
        let md = "# Postmortem: checkout outage, 14 June 2026\n\n\
                  ## Impact\n\
                  Checkout was unavailable for 51 minutes.\n\n\
                  ## Root cause\n\
                  The payment gateway TLS certificate expired.\n\n\
                  ## Resolution\n\
                  We reloaded the service and added an alert.\n";
        assert_eq!(kind(md), Family::TxtProse);
    }

    #[test]
    fn short_runbook_is_still_prose() {
        let md = "# Database runbook\n\n\
                  ## Failover\n\
                  Promote the standby with pg_ctl promote.\n\n\
                  ## Pool exhaustion\n\
                  Symptoms are rising p99 and pool errors in the logs.\n";
        assert_eq!(kind(md), Family::TxtProse);
    }

    /// The record-stream side must be unaffected — these are what TxtLines is for.
    #[test]
    fn access_logs_stay_line_records() {
        let log = (0..20)
            .map(|i| format!(
                "10.0.0.{i} - - [01/Jun/2026:10:00:00 +0000] \"GET /api/checkout HTTP/1.1\" 200 {i}00 \"-\" \"Mozilla/5.0\""
            ))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(classify_full(&log), Family::Logs);
    }

    #[test]
    fn syslog_stays_line_records() {
        // One message in five ends with a period — well under the threshold.
        let msgs = [
            "sshd[123]: Accepted publickey for deploy from 10.0.3.4 port 55212",
            "kernel: Out of memory: Killed process 8123 (java)",
            "cron[99]: session opened for user root by (uid=0)",
            "postfix[7]: connection timed out while talking to upstream",
            "systemd[1]: Started Daily apt download activities.",
        ];
        let log = (0..6)
            .flat_map(|_| msgs.iter().map(|m| format!("Jun  1 10:00:00 host1 {m}")))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(classify_full(&log), Family::Logs);
    }

    #[test]
    fn source_code_stays_line_records() {
        let code = "pub struct Pool { max: usize, in_use: usize }\n\
                    impl Pool {\n\
                    pub fn acquire(&mut self) -> Result<Conn, PoolError> {\n\
                    if self.in_use >= self.max { return Err(PoolError::Exhausted); }\n\
                    self.in_use += 1;\n\
                    Ok(Conn::new())\n\
                    }\n\
                    }\n\
                    fn helper() -> u32 { 42 }\n\
                    const LIMIT: usize = 10;\n";
        assert_eq!(kind(code), Family::TxtLines);
    }

    #[test]
    fn long_lines_are_prose_regardless_of_punctuation() {
        let t = (0..10)
            .map(|_| "x".repeat(120))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(kind(&t), Family::TxtProse);
    }

    /// Regression: a markdown checklist (`- [ ]` task items under a heading)
    /// used to sniff as YAML — 2 of 3 nonblank lines start with `- ` — and
    /// the YAML extractor then junk-filed it (and, before the yaml_x
    /// non-progress fix, hung on it). Checkbox items are invalid YAML and
    /// must not count as YAML evidence.
    #[test]
    fn markdown_checklist_is_prose_not_yaml() {
        let md = "# Launch checklist\n\n\
                  - [ ] Sign off the [business plan](01-business-plan.md)\n\
                  - [x] Close out permits\n\
                  - [ ] Dry run: two full batches back to back\n";
        assert_eq!(classify_full(md), Family::TxtProse);
        // A real YAML list is still YAML.
        let yaml = "- alpha\n- beta\n- gamma\n";
        assert_eq!(classify_full(yaml), Family::Yaml);
    }

    /// Regression (second-brain demo vault, live 2026-07-30): a markdown
    /// note hard-wrapped at ~75 columns averages < 60 chars/line and ends
    /// most lines mid-sentence, so it scored below the 0.40 sentence ratio
    /// and landed in TxtLines — silently losing its title, its `s0` section
    /// anchor, and its wikilink detection (8 of 39 vault files). A file that
    /// opens with an ATX heading and shows markdown link syntax (or some
    /// terminal punctuation) is markdown prose.
    #[test]
    fn hard_wrapped_markdown_note_is_prose_not_lines() {
        let md = "# Hydration\n\n\
                  Hydration is water as a percentage of flour weight, the core of\n\
                  [[baker-percentages]]. 65% is a tight sandwich loaf; 75-80% is where open\n\
                  [[crumb-structure]] lives; past 85% the dough demands real skill.\n\n\
                  Higher hydration = looser dough, longer bulk, bigger holes. It is the single\n\
                  most consequential number in the formula.\n";
        assert_eq!(classify_full(md), Family::TxtProse);
        // No heading opener → the rescue must not fire: a Python file whose
        // comment banner starts mid-file stays wherever the base heuristics
        // put it, and a shebang is not a heading.
        let sh = "#!/usr/bin/env bash\n\
                  set -euo pipefail\n\
                  for f in a b c; do\n\
                  echo one\n\
                  echo two\n\
                  echo three\n\
                  done\n";
        assert_eq!(classify_full(sh), Family::TxtLines);
    }
}
