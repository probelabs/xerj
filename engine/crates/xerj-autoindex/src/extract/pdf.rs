//! PDF text extraction backed by a real PDF parser.
//!
//! The old implementation searched raw streams and interpreted font character
//! codes as Latin-1. `pdf_oxide` resolves the object graph and font mappings
//! before returning Unicode text. Each document runs in a fresh worker process
//! so parser state or failure cannot contaminate the server. On Unix the worker
//! also gets a process group and an address-space limit. This is crash/resource
//! isolation, not a security sandbox: the worker retains the user's authority.

use super::{split_sections, ExtractStats, RawRecord, Sink};
use anyhow::{anyhow, Context, Result};
use pdf_oxide::PdfDocument;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const PDF_CAP: u64 = 512 << 20;
const MAX_PAGES: usize = 100_000;
const MAX_PAGE_TEXT: usize = 16 << 20;
const MAX_EXTRACTED_TEXT: usize = 64 << 20;
const MAX_WORKER_OUTPUT: usize = 32 << 20;
const MAX_WORKER_STDERR: u64 = 64 << 10;
const WORKER_ADDRESS_SPACE: u64 = 1536 << 20;

pub fn extract(path: &Path, sink: Sink) -> Result<ExtractStats> {
    let _permit = worker_gate().acquire();
    let response = spawn_worker(path)?;
    Ok(deliver(response, sink))
}

/// Run the isolated parser once and retain its bounded protocol response in an
/// anonymous, run-local file for Phase B.
///
/// The spool is deliberately not durable state. A process restart has a frozen
/// inference plan but no trusted open handle, so it parses the PDF again. This
/// avoids coupling extraction cache recovery to the publication journal.
pub(crate) fn extract_and_spool(
    path: &Path,
    state_dir: &Path,
    source_size: u64,
    source_digest: &str,
    budget: &Arc<ExtractionSpoolBudget>,
    sink: Sink,
) -> Result<(ExtractStats, Option<ExtractionSpool>, Option<String>)> {
    let _permit = worker_gate().acquire();
    let response = spawn_worker(path)?;
    let (spool, fallback) =
        try_spool_response(state_dir, source_size, source_digest, budget, &response);
    let stats = deliver(response, sink);
    Ok((stats, spool, fallback))
}

fn try_spool_response(
    state_dir: &Path,
    source_size: u64,
    source_digest: &str,
    budget: &Arc<ExtractionSpoolBudget>,
    response: &WorkerResponse,
) -> (Option<ExtractionSpool>, Option<String>) {
    let reservation = budget.try_reserve(MAX_WORKER_OUTPUT as u64);
    match reservation {
        Some(reservation) => {
            match spool_response(state_dir, source_size, source_digest, response, reservation) {
                Ok(spool) => (Some(spool), None),
                Err(error) => (
                    None,
                    Some(format!(
                        "could not retain the run-local extraction artifact: {error:#}"
                    )),
                ),
            }
        }
        None => (
            None,
            Some(format!(
                "the run-local extraction budget is full ({} MiB or {} artifacts)",
                budget.limit >> 20,
                budget.max_spools
            )),
        ),
    }
}

fn spool_response(
    state_dir: &Path,
    source_size: u64,
    source_digest: &str,
    response: &WorkerResponse,
    mut reservation: ExtractionSpoolReservation,
) -> Result<ExtractionSpool> {
    validate_response_identity(response)?;
    let mut file = tempfile::tempfile_in(state_dir).with_context(|| {
        format!(
            "create anonymous PDF extraction spool under {}",
            state_dir.display()
        )
    })?;
    {
        let mut writer = BufWriter::new(&mut file);
        serde_json::to_writer(&mut writer, &response)
            .context("serialize validated PDF extraction spool")?;
        writer.flush().context("flush PDF extraction spool")?;
    }
    let bytes = file
        .stream_position()
        .context("measure PDF extraction spool")?;
    if bytes > MAX_WORKER_OUTPUT as u64 {
        return Err(anyhow!(
            "validated PDF extraction spool exceeded the {} MiB parent-memory safety limit",
            MAX_WORKER_OUTPUT >> 20
        ));
    }
    reservation.shrink_to(bytes);
    file.rewind().context("rewind PDF extraction spool")?;
    Ok(ExtractionSpool {
        file: Mutex::new(file),
        source_size,
        source_digest: source_digest.to_owned(),
        bytes,
        _reservation: reservation,
    })
}

pub(crate) struct ExtractionSpoolBudget {
    used: AtomicU64,
    spools: AtomicU64,
    limit: u64,
    max_spools: u64,
}

impl ExtractionSpoolBudget {
    pub(crate) fn new(limit: u64, max_spools: u64) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicU64::new(0),
            spools: AtomicU64::new(0),
            limit,
            max_spools,
        })
    }

    fn try_reserve(self: &Arc<Self>, bytes: u64) -> Option<ExtractionSpoolReservation> {
        self.spools
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |spools| {
                (spools < self.max_spools).then_some(spools + 1)
            })
            .ok()?;
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            let Some(next) = used.checked_add(bytes) else {
                self.spools.fetch_sub(1, Ordering::AcqRel);
                return None;
            };
            if next > self.limit {
                self.spools.fetch_sub(1, Ordering::AcqRel);
                return None;
            }
            match self
                .used
                .compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    return Some(ExtractionSpoolReservation {
                        budget: Arc::clone(self),
                        bytes,
                    });
                }
                Err(actual) => used = actual,
            }
        }
    }
}

struct ExtractionSpoolReservation {
    budget: Arc<ExtractionSpoolBudget>,
    bytes: u64,
}

impl ExtractionSpoolReservation {
    fn shrink_to(&mut self, bytes: u64) {
        debug_assert!(bytes <= self.bytes);
        let released = self.bytes - bytes;
        self.bytes = bytes;
        let previous = self.budget.used.fetch_sub(released, Ordering::AcqRel);
        debug_assert!(previous >= released);
    }
}

impl Drop for ExtractionSpoolReservation {
    fn drop(&mut self) {
        let previous = self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
        debug_assert!(previous >= self.bytes);
        let previous_spools = self.budget.spools.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous_spools > 0);
    }
}

/// A validated worker response bound to one inventory generation.
///
/// `File` is anonymous and the mutex owns its single seek cursor. Phase A and
/// Phase B are sequential today, but cursor ownership remains explicit rather
/// than relying on cloned Unix file descriptors with shared offsets.
pub(crate) struct ExtractionSpool {
    file: Mutex<File>,
    source_size: u64,
    source_digest: String,
    bytes: u64,
    _reservation: ExtractionSpoolReservation,
}

impl ExtractionSpool {
    pub(crate) fn replay(
        &self,
        source_size: u64,
        source_digest: &str,
        sink: Sink,
    ) -> Result<ExtractStats> {
        anyhow::ensure!(
            source_size == self.source_size && source_digest == self.source_digest,
            "PDF extraction spool belongs to a different source generation; retry extraction"
        );
        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow!("PDF extraction spool lock was poisoned"))?;
        file.rewind().context("rewind PDF extraction spool")?;
        let response: WorkerResponse = serde_json::from_reader(BufReader::new(&mut *file))
            .context("PDF extraction spool is malformed or truncated; retry extraction")?;
        validate_response_identity(&response)?;
        anyhow::ensure!(
            file.stream_position()? <= self.bytes,
            "PDF extraction spool exceeded its validated boundary"
        );
        Ok(deliver(response, sink))
    }
}

fn deliver(response: WorkerResponse, sink: Sink) -> ExtractStats {
    let mut stats = ExtractStats::default();
    for record in response.records {
        stats.records += 1;
        if !sink(record) {
            break;
        }
    }
    stats
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerResponse {
    schema: u32,
    extractor: String,
    parser: String,
    containment: String,
    records: Vec<RawRecord>,
}

pub fn configure_workers(workers: usize) {
    worker_gate().set_limit(workers.clamp(1, 4));
}

pub fn configure_timeout(seconds: u64) {
    WORKER_TIMEOUT_SECS.store(seconds.clamp(1, 3600), Ordering::Relaxed);
}

/// Hidden same-binary worker entry point. One invocation parses one document.
pub fn run_worker_cli() -> i32 {
    let mut args = std::env::args_os().skip(2);
    let Some(path) = args.next().map(PathBuf::from) else {
        eprintln!("PDF worker protocol error: missing input path");
        return 2;
    };
    if args.next().is_some() {
        eprintln!("PDF worker protocol error: unexpected arguments");
        return 2;
    }
    match extract_in_process(&path) {
        Ok(records) => {
            let response = WorkerResponse {
                schema: 1,
                extractor: format!("xerj-autoindex/{}", env!("CARGO_PKG_VERSION")),
                parser: format!("pdf_oxide/{}", pdf_oxide::VERSION),
                containment: containment_description().to_string(),
                records,
            };
            let result = (|| -> Result<()> {
                let mut stdout = std::io::stdout().lock();
                serde_json::to_writer(&mut stdout, &response)?;
                stdout.flush()?;
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("PDF worker could not write its bounded result: {error}");
                return 1;
            }
            0
        }
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    }
}

fn spawn_worker(path: &Path) -> Result<WorkerResponse> {
    // Trusted developer/test hook. The selected executable runs with the
    // invoking user's authority and must implement this private protocol.
    let executable = std::env::var_os("XERJ_PDF_WORKER_BIN")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .context("cannot locate the xerj executable for isolated PDF extraction")?;
    let mut command = Command::new(executable);
    command
        .arg("__extract-pdf")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    set_worker_memory_limit(&mut command);
    let mut child = command.spawn().with_context(|| {
        format!(
            "could not start the isolated PDF parser for {}; check process/resource limits",
            path.display()
        )
    })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || read_capped(stdout, MAX_WORKER_OUTPUT));
    let err_reader = std::thread::spawn(move || read_capped(stderr, MAX_WORKER_STDERR as usize));

    let started = Instant::now();
    let timeout = Duration::from_secs(WORKER_TIMEOUT_SECS.load(Ordering::Relaxed));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                terminate_worker_tree(&mut child);
                let _ = out_reader.join();
                let _ = err_reader.join();
                return Err(error).with_context(|| {
                    format!("could not monitor PDF parser for {}", path.display())
                });
            }
        }
        if started.elapsed() >= timeout {
            terminate_worker_tree(&mut child);
            let _ = out_reader.join();
            let _ = err_reader.join();
            return Err(anyhow!(
                "PDF extraction timed out after {} seconds for {}; repair/split the PDF, or run OCR if it is image-only",
                timeout.as_secs(),
                path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    terminate_worker_descendants(child.id());
    let stdout = out_reader
        .join()
        .map_err(|_| anyhow!("PDF parser output reader panicked"))??;
    let stderr = err_reader
        .join()
        .map_err(|_| anyhow!("PDF parser error reader panicked"))??;
    if stdout.overflow {
        return Err(anyhow!(
            "PDF parser output exceeded the {} MiB parent-memory safety limit for {}; split the document before indexing",
            MAX_WORKER_OUTPUT >> 20,
            path.display()
        ));
    }
    if stderr.overflow {
        return Err(anyhow!(
            "PDF parser error output exceeded the {} KiB safety limit for {}; the worker was excessively noisy and its truncated diagnostic is: {}",
            MAX_WORKER_STDERR >> 10,
            path.display(),
            String::from_utf8_lossy(&stderr.prefix)
        ));
    }
    let stderr = String::from_utf8_lossy(&stderr.prefix);
    if !status.success() {
        return Err(anyhow!(
            "isolated PDF parser failed for {}{}{}; verify/decrypt the PDF, repair it, or run OCR for image-only input",
            path.display(),
            if stderr.trim().is_empty() { "" } else { ": " },
            stderr.trim()
        ));
    }
    let response: WorkerResponse = serde_json::from_slice(&stdout.prefix).with_context(|| {
        format!(
            "isolated PDF parser returned an invalid result for {}; this is an internal worker-protocol error",
            path.display()
        )
    })?;
    validate_response_identity(&response)?;
    Ok(response)
}

fn validate_response_identity(response: &WorkerResponse) -> Result<()> {
    if response.schema != 1 {
        return Err(anyhow!(
            "PDF parser returned unsupported extraction schema {}; update parent and worker together",
            response.schema
        ));
    }
    let expected_extractor = format!("xerj-autoindex/{}", env!("CARGO_PKG_VERSION"));
    if response.extractor != expected_extractor {
        return Err(anyhow!(
            "PDF extractor version mismatch: expected {expected_extractor}, worker reported {}; update parent and worker together",
            response.extractor
        ));
    }
    if response.parser != format!("pdf_oxide/{}", pdf_oxide::VERSION) {
        return Err(anyhow!(
            "PDF parser version mismatch: expected pdf_oxide/{}, worker reported {}; update parent and worker together",
            pdf_oxide::VERSION, response.parser
        ));
    }
    Ok(())
}

static WORKER_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(120);

struct CappedRead {
    prefix: Vec<u8>,
    overflow: bool,
}

fn read_capped(mut input: impl Read, cap: usize) -> Result<CappedRead> {
    let mut prefix = Vec::with_capacity(cap.min(64 << 10));
    let mut overflow = false;
    let mut chunk = [0u8; 64 << 10];
    loop {
        let read = input
            .read(&mut chunk)
            .context("could not read isolated PDF parser output")?;
        if read == 0 {
            break;
        }
        let remaining = cap.saturating_sub(prefix.len());
        let retain = remaining.min(read);
        prefix.extend_from_slice(&chunk[..retain]);
        overflow |= retain < read;
        // Keep draining after overflow so the child can exit instead of
        // blocking forever on a full pipe.
    }
    Ok(CappedRead { prefix, overflow })
}

#[cfg(unix)]
fn set_worker_memory_limit(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let limit = libc::rlimit {
                rlim_cur: WORKER_ADDRESS_SPACE,
                rlim_max: WORKER_ADDRESS_SPACE,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn set_worker_memory_limit(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_worker_tree(child: &mut std::process::Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_worker_descendants(process_group: u32) {
    unsafe {
        libc::kill(-(process_group as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn terminate_worker_tree(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate_worker_descendants(_process_group: u32) {}

#[cfg(unix)]
fn containment_description() -> &'static str {
    "fresh process; Unix process-group cleanup; 1536 MiB RLIMIT_AS; not a security sandbox"
}

#[cfg(not(unix))]
fn containment_description() -> &'static str {
    "fresh process; no OS memory cap on this platform; not a security sandbox"
}

fn extract_in_process(path: &Path) -> Result<Vec<RawRecord>> {
    let size = std::fs::metadata(path)?.len();
    if size > PDF_CAP {
        return Err(anyhow!(
            "PDF is {:.1} MiB, above the 512 MiB safety limit; split or compress it before indexing",
            size as f64 / (1 << 20) as f64
        ));
    }

    pdf_oxide::fonts::global_cache::clear_global_font_cache();
    let doc = PdfDocument::open(path).with_context(|| {
        "PDF parser could not open the document; verify/repair damaged input, decrypt password-protected input, or run OCR for image-only input"
    })?;
    let pages = doc.page_count().with_context(|| {
        "PDF parser could not read the page tree; verify the file or decrypt it first"
    })?;
    if pages == 0 {
        return Err(anyhow!("PDF contains no pages"));
    }
    if pages > MAX_PAGES {
        return Err(anyhow!(
            "PDF declares {pages} pages, above the {MAX_PAGES}-page safety limit; split it before indexing"
        ));
    }

    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "untitled".to_string());
    let mut records = Vec::new();
    let mut extracted_bytes = 0usize;
    let mut failed_pages = Vec::new();
    let mut omitted_pages = Vec::new();
    let mut pages_with_text = 0usize;

    for page_index in 0..pages {
        let page = match doc.extract_text(page_index) {
            Ok(text) => text,
            Err(error) => {
                failed_pages.push(format!("{} ({error})", page_index + 1));
                continue;
            }
        };
        let page = normalize_legacy_symbol_glyphs(&page);
        let body = page.trim();
        if body.is_empty() {
            omitted_pages.push(page_index + 1);
            continue;
        }
        if body.len() > MAX_PAGE_TEXT {
            return Err(anyhow!(
                "PDF page {} produced more than 16 MiB of text; split or repair the document before indexing",
                page_index + 1
            ));
        }
        if body.chars().filter(|ch| !ch.is_whitespace()).count() < 8 {
            omitted_pages.push(page_index + 1);
            continue;
        }
        validate_text_quality(body).with_context(|| {
            format!(
                "PDF page {} produced untrusted text; refusing to index possible binary/font garbage",
                page_index + 1
            )
        })?;
        extracted_bytes = extracted_bytes.saturating_add(body.len());
        pages_with_text += 1;
        if extracted_bytes > MAX_EXTRACTED_TEXT {
            return Err(anyhow!(
                "PDF extraction exceeded the 64 MiB text safety limit; split the document before indexing"
            ));
        }

        for (section, text) in split_sections(body).into_iter().enumerate() {
            let mut fields = Map::new();
            fields.insert("title".into(), Value::String(title.clone()));
            fields.insert(
                "page".into(),
                Value::Number(((page_index + 1) as u64).into()),
            );
            if section > 0 {
                fields.insert("section".into(), Value::Number((section as u64).into()));
            }
            fields.insert("body".into(), Value::String(text));
            records.push(RawRecord {
                fields,
                locator: format!("p{}-s{}", page_index + 1, section),
                group: None,
            });
        }
    }

    if records.is_empty() {
        if failed_pages.is_empty() {
            return Err(anyhow!(
                "PDF has no extractable text; it may be an image-only scan. Run OCR first, then autoindex the searchable PDF"
            ));
        }
        return Err(anyhow!(
            "PDF text extraction failed on every non-empty page ({}); verify/decrypt the PDF or run OCR before indexing",
            failed_pages.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if !failed_pages.is_empty() {
        return Err(anyhow!(
            "PDF extraction failed on {}/{} pages (first failures: {}); refusing a partial index. Repair/decrypt the PDF or run OCR first",
            failed_pages.len(),
            pages,
            failed_pages.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    for record in &mut records {
        record.fields.insert(
            "pdf_pages_total".into(),
            Value::Number((pages as u64).into()),
        );
        record.fields.insert(
            "pdf_pages_with_text".into(),
            Value::Number((pages_with_text as u64).into()),
        );
        record.fields.insert(
            "pdf_pages_omitted".into(),
            Value::Number((omitted_pages.len() as u64).into()),
        );
        if !omitted_pages.is_empty() {
            record.fields.insert(
                "pdf_ocr_warning".into(),
                Value::String(format!(
                    "{} page(s) had no usable text (first: {}); intentional blanks or scanned pages may need OCR",
                    omitted_pages.len(),
                    omitted_pages
                        .iter()
                        .take(8)
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            );
        }
    }
    Ok(records)
}

struct WorkerGate {
    state: Mutex<(usize, usize)>,
    ready: Condvar,
}

impl WorkerGate {
    fn set_limit(&self, limit: usize) {
        self.state.lock().expect("PDF worker gate poisoned").1 = limit;
        self.ready.notify_all();
    }

    fn acquire(&self) -> WorkerPermit<'_> {
        let mut state = self.state.lock().expect("PDF worker gate poisoned");
        while state.0 >= state.1 {
            state = self.ready.wait(state).expect("PDF worker gate poisoned");
        }
        state.0 += 1;
        WorkerPermit(self)
    }
}

struct WorkerPermit<'a>(&'a WorkerGate);

impl Drop for WorkerPermit<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("PDF worker gate poisoned");
        state.0 -= 1;
        self.0.ready.notify_one();
    }
}

fn worker_gate() -> &'static WorkerGate {
    static GATE: OnceLock<WorkerGate> = OnceLock::new();
    GATE.get_or_init(|| WorkerGate {
        state: Mutex::new((
            0,
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(4),
        )),
        ready: Condvar::new(),
    })
}

fn normalize_legacy_symbol_glyphs(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\u{f0b7}' => '•',
            '\u{f02d}' => '-',
            '\u{f078}' | '\u{f0a8}' | '\u{f06f}' | '\u{f052}' | '\u{f0a3}' | '\u{f020}' => '□',
            '\u{f0d2}' | '\u{f0d4}' | '\u{f0be}' => ' ',
            other => other,
        })
        .collect()
}

fn validate_text_quality(text: &str) -> Result<()> {
    // Fail closed rather than embed broken font codes. On the pinned
    // 368-document FinanceBench PDF corpus this 0.5% threshold accepted 367
    // documents and rejected one; keep that false-rejection cost visible when
    // recalibrating against broader labeled corpora.
    let mut visible = 0usize;
    let mut semantic = 0usize;
    let mut suspicious = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        visible += 1;
        if ch.is_alphanumeric() {
            semantic += 1;
        }
        if ch == '\u{fffd}'
            || ch.is_control()
            || ('\u{e000}'..='\u{f8ff}').contains(&ch)
            || ('\u{f0000}'..='\u{ffffd}').contains(&ch)
            || ('\u{100000}'..='\u{10fffd}').contains(&ch)
        {
            suspicious += 1;
        }
    }
    if visible < 8 {
        return Err(anyhow!("too little visible text"));
    }
    if suspicious * 200 > visible {
        return Err(anyhow!(
            "more than 0.5% of visible characters are controls, replacement glyphs, or private-use codes"
        ));
    }
    if semantic * 20 < visible {
        return Err(anyhow!(
            "fewer than 5% of visible characters are letters or numbers"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        containment_description, extract_in_process, normalize_legacy_symbol_glyphs,
        spool_response, try_spool_response, validate_text_quality, WorkerResponse,
    };
    use crate::infer::{infer_fields, FieldAcc};
    use serde_json::Value;
    use std::collections::HashMap;
    use std::io::{Seek, Write};

    fn write_prose_pdf(path: &std::path::Path) {
        let prose = "Quarterly revenue increased because subscription demand remained strong. \
            Operating income improved as cloud infrastructure costs declined. Management expects \
            cash flow to support continued investment throughout the next fiscal year.";
        let stream = format!("BT /F1 12 Tf 72 720 Td ({prose}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        std::fs::write(path, pdf).unwrap();
    }

    #[test]
    fn quality_gate_accepts_financial_and_international_text() {
        validate_text_quality(
            "Consolidated revenue — Q4 2025\n€1,234.5 million (增长率 12%; прибыль 8%).",
        )
        .unwrap();
    }

    #[test]
    fn quality_gate_rejects_binary_and_font_garbage() {
        let garbage = "abc\u{0000}\u{0001}\u{fffd}\u{e123}\u{0002}xyz";
        assert!(validate_text_quality(garbage).is_err());
    }

    #[test]
    fn quality_gate_rejects_operator_noise() {
        assert!(validate_text_quality("//// <<>> [] () -- === +++").is_err());
    }

    #[test]
    fn known_legacy_symbol_glyphs_are_normalized_but_unknown_pua_is_rejected() {
        let normalized = normalize_legacy_symbol_glyphs(
            "Yes \u{f078} No \u{f0a8}\n\u{f0b7} Revenue\nrisk\u{f02d}adjusted",
        );
        assert_eq!(normalized, "Yes □ No □\n• Revenue\nrisk-adjusted");
        validate_text_quality(&normalized).unwrap();
        assert!(validate_text_quality("Revenue \u{e123} remains unknown").is_err());
    }

    #[test]
    fn reproducible_pdf_extracts_pages_and_elects_semantic_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quarterly-report.pdf");
        write_prose_pdf(&path);

        let first = extract_in_process(&path).unwrap();
        let second = extract_in_process(&path).unwrap();
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second).unwrap()
        );
        assert!(!first.is_empty());
        assert_eq!(first[0].fields["page"], 1);
        assert!(first[0].locator.starts_with("p1-s"));
        assert!(first[0].fields["body"]
            .as_str()
            .unwrap()
            .contains("Quarterly revenue increased"));

        let mut fields: HashMap<String, FieldAcc> = HashMap::new();
        for record in &first {
            for (name, value) in &record.fields {
                fields.entry(name.clone()).or_default().add(value);
            }
        }
        let specs = infer_fields(&fields, first.len() as u64, false);
        let body = specs.iter().find(|spec| spec.name == "body").unwrap();
        assert_eq!(body.es_type, "semantic_text");
        assert!(matches!(
            first[0].fields.get("body"),
            Some(Value::String(_))
        ));
    }

    fn response(records: Vec<crate::extract::RawRecord>) -> WorkerResponse {
        WorkerResponse {
            schema: 1,
            extractor: format!("xerj-autoindex/{}", env!("CARGO_PKG_VERSION")),
            parser: format!("pdf_oxide/{}", pdf_oxide::VERSION),
            containment: containment_description().to_string(),
            records,
        }
    }

    #[test]
    fn anonymous_spool_replays_exact_records_and_rejects_another_generation() {
        let source = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = source.path().join("quarterly-report.pdf");
        write_prose_pdf(&path);
        let expected = extract_in_process(&path).unwrap();
        let budget = super::ExtractionSpoolBudget::new(super::MAX_WORKER_OUTPUT as u64, 1);
        let spool = spool_response(
            state.path(),
            123,
            "axf2-generation-a",
            &response(expected.clone()),
            budget.try_reserve(super::MAX_WORKER_OUTPUT as u64).unwrap(),
        )
        .unwrap();

        // The artifact exists only as an open handle; it adds no recoverable
        // path or stale-cache state under the journal directory.
        assert_eq!(std::fs::read_dir(state.path()).unwrap().count(), 0);

        let mut replayed = Vec::new();
        let stats = spool
            .replay(123, "axf2-generation-a", &mut |record| {
                replayed.push(record);
                true
            })
            .unwrap();
        assert_eq!(stats.records as usize, expected.len());
        assert_eq!(
            serde_json::to_value(&replayed).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );

        let error = spool
            .replay(123, "axf2-generation-b", &mut |_| true)
            .unwrap_err();
        assert!(error.to_string().contains("different source generation"));
    }

    #[test]
    fn spool_replay_fails_closed_on_truncation_and_wrong_worker_identity() {
        let source = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = source.path().join("quarterly-report.pdf");
        write_prose_pdf(&path);
        let records = extract_in_process(&path).unwrap();

        let budget = super::ExtractionSpoolBudget::new(2 * super::MAX_WORKER_OUTPUT as u64, 2);
        let truncated = spool_response(
            state.path(),
            7,
            "digest",
            &response(records.clone()),
            budget.try_reserve(super::MAX_WORKER_OUTPUT as u64).unwrap(),
        )
        .unwrap();
        truncated.file.lock().unwrap().set_len(8).unwrap();
        let error = truncated.replay(7, "digest", &mut |_| true).unwrap_err();
        assert!(format!("{error:#}").contains("malformed or truncated"));

        let wrong_identity = spool_response(
            state.path(),
            7,
            "digest",
            &response(records.clone()),
            budget.try_reserve(super::MAX_WORKER_OUTPUT as u64).unwrap(),
        )
        .unwrap();
        let mut invalid = response(records);
        invalid.parser = "pdf_oxide/not-the-pinned-parser".into();
        {
            let mut file = wrong_identity.file.lock().unwrap();
            file.rewind().unwrap();
            file.set_len(0).unwrap();
            serde_json::to_writer(&mut *file, &invalid).unwrap();
            file.flush().unwrap();
        }
        let error = wrong_identity
            .replay(7, "digest", &mut |_| true)
            .unwrap_err();
        assert!(error.to_string().contains("PDF parser version mismatch"));
    }

    #[test]
    fn spool_budget_is_atomic_and_refunded_on_physical_drop() {
        let budget = super::ExtractionSpoolBudget::new(100, 2);
        let mut first = budget.try_reserve(80).unwrap();
        assert!(budget.try_reserve(21).is_none());
        first.shrink_to(40);
        let second = budget.try_reserve(60).unwrap();
        assert!(budget.try_reserve(1).is_none());
        drop(second);
        assert!(budget.try_reserve(60).is_some());
        drop(first);

        let count_budget = super::ExtractionSpoolBudget::new(1_000, 1);
        let only = count_budget.try_reserve(1).unwrap();
        assert!(count_budget.try_reserve(1).is_none());
        drop(only);
        assert!(count_budget.try_reserve(1).is_some());
    }

    #[test]
    fn spool_io_failure_preserves_a_clean_fallback_and_refunds_capacity() {
        let state = tempfile::tempdir().unwrap();
        let not_a_directory = state.path().join("regular-file");
        std::fs::write(&not_a_directory, b"not a directory").unwrap();
        let budget = super::ExtractionSpoolBudget::new(super::MAX_WORKER_OUTPUT as u64, 1);
        let response = response(Vec::new());

        let (spool, fallback) =
            try_spool_response(&not_a_directory, 7, "digest", &budget, &response);
        assert!(spool.is_none());
        let fallback = fallback.unwrap();
        assert!(fallback.contains("could not retain"));
        assert!(fallback.contains("anonymous PDF extraction spool"));

        // The failed attempt must not strand either the byte or handle charge.
        assert!(budget
            .try_reserve(super::MAX_WORKER_OUTPUT as u64)
            .is_some());
    }
}
