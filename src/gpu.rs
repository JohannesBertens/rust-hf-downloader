//! GPU platform detection for FLARE payload revisions
//! (plans/flare-models.md §3.2).
//!
//! FLARE packages are compiled per NVIDIA architecture; the payload branch
//! to download is selected by mapping the local GPU's PCI device id onto a
//! platform ([`FlarePlatform`]). The mapping table is flare's own, byte for
//! byte (source: `helios/src/package/naming.rs platform_from_pci`; detection
//! flow: `flare/src/hardware.rs detect_isolated`):
//!
//! - `0x2E12` → `gb10`
//! - `{0x2B85, 0x2B87, 0x2B8C, 0x2BB1, 0x2BB2, 0x2BB3, 0x2BB4, 0x2BB5, 0x2BB9}` → `gb202`
//! - anything else — notably GB203 (`0x2C02`) and GB205 (`0x2F04`) — maps to
//!   nothing: flare's rule is to demand an explicit revision instead of
//!   guessing, never to ship a broken package.
//!
//! Detection shells out to `nvidia-smi --query-gpu=…` once (5-second budget)
//! and keeps every decision pure: [`platform_from_query_rows`] holds the
//! whole decision logic and is unit-tested from inline CSV fixtures, so the
//! subprocess shell stays thin. The module is dependency-free (std only);
//! the sync [`detect_platform`] is meant to be wrapped in
//! `tokio::task::spawn_blocking` by async callers.
//!
//! This is a wave-1 contract surface (plans/flare-models.md §7/§8): the
//! consumer wiring (CLI `--for flare`/`--gpus`, TUI platform star) lands in
//! wave 2, which is why the module-level `dead_code` allowance exists until
//! then.

#![allow(dead_code)]

use std::fmt;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// NVIDIA PCI vendor id — the low 16-bit half of a 32-bit `pci.device_id`
/// rendering; never the platform key itself.
const NVIDIA_VENDOR_ID: u16 = 0x10DE;

/// Wall-clock budget for one `nvidia-smi` invocation
/// (plans/flare-models.md §3.2: 5-second timeout).
const NVIDIA_SMI_TIMEOUT: Duration = Duration::from_secs(5);

/// The FLARE payload platform a package is compiled for
/// (plans/flare-models.md §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlarePlatform {
    /// Blackwell GB202 family.
    Gb202,
    /// Blackwell GB10 (DGX Spark class).
    Gb10,
}

impl FlarePlatform {
    /// flare's canonical platform token (its `naming.rs` strings).
    pub fn as_str(&self) -> &'static str {
        match self {
            FlarePlatform::Gb202 => "gb202",
            FlarePlatform::Gb10 => "gb10",
        }
    }
}

/// flare's exact PCI device-id table (`helios/src/package/naming.rs
/// platform_from_pci`), byte for byte. Anything outside the table —
/// notably GB203 (`0x2C02`) and GB205 (`0x2F04`) — is [`None`]: the caller
/// must demand an explicit revision rather than guess.
pub fn platform_from_device_id(id: u16) -> Option<FlarePlatform> {
    match id {
        0x2E12 => Some(FlarePlatform::Gb10),
        0x2B85 | 0x2B87 | 0x2B8C | 0x2BB1 | 0x2BB2 | 0x2BB3 | 0x2BB4 | 0x2BB5 | 0x2BB9 => {
            Some(FlarePlatform::Gb202)
        }
        _ => None,
    }
}

/// Parse a `--gpus` specification: comma-separated integer indices and
/// inclusive `N-M` ranges (plans/flare-models.md §3.2, flare's grammar).
///
/// - empty / whitespace-only input ⇒ `Err`;
/// - an inverted range (`M < N`) ⇒ `Err`;
/// - duplicates collapse silently (first occurrence wins, user order kept);
/// - tokens that look like GPU/MIG UUIDs are rejected with a message
///   pointing at `--revision` (rhd has no UUID selector — the revision must
///   be pinned explicitly).
pub fn parse_gpus_spec(s: &str) -> Result<Vec<usize>, String> {
    let spec = s.trim();
    if spec.is_empty() {
        return Err(
            "empty --gpus spec: expected comma-separated GPU indices or N-M ranges".to_string(),
        );
    }
    let mut out: Vec<usize> = Vec::new();
    for token in spec.split(',') {
        let token = token.trim();
        if token.is_empty() {
            return Err(
                "invalid --gpus token \"\": empty entry (expected an index or N-M range)"
                    .to_string(),
            );
        }
        if let Some((start, end)) = parse_range_token(token) {
            if end < start {
                return Err(format!(
                    "invalid --gpus range \"{token}\": start {start} exceeds end {end}"
                ));
            }
            for idx in start..=end {
                if !out.contains(&idx) {
                    out.push(idx);
                }
            }
        } else if let Ok(idx) = token.parse::<usize>() {
            if !out.contains(&idx) {
                out.push(idx);
            }
        } else {
            return Err(gpus_token_error(token));
        }
    }
    Ok(out)
}

/// An inclusive `N-M` range token: both halves plain non-negative integers.
fn parse_range_token(token: &str) -> Option<(usize, usize)> {
    let (start, end) = token.split_once('-')?;
    let start = start.trim().parse::<usize>().ok()?;
    let end = end.trim().parse::<usize>().ok()?;
    Some((start, end))
}

/// Error text for a `--gpus` token that is neither an index nor a range.
/// UUID-shaped tokens get an explicit hint: flare's `GPU-…`/`MIG-…`
/// selector has no rhd equivalent — the revision must be pinned explicitly
/// (the CLI layer owns the concrete "available revisions: …" listing).
fn gpus_token_error(token: &str) -> String {
    if is_uuidish_token(token) {
        format!(
            "invalid --gpus token \"{token}\": GPU/MIG UUID selectors are not supported — pass an explicit --revision instead"
        )
    } else {
        format!(
            "invalid --gpus token \"{token}\": expected a GPU index or an inclusive N-M range of indices"
        )
    }
}

/// UUID-ish: flare's `GPU-…`/`MIG-…` selectors, or a bare canonical UUID
/// (8-4-4-4-12 hex groups). Used only to pick the error wording.
fn is_uuidish_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("gpu-") || lower.starts_with("mig-") {
        return true;
    }
    let shapes = [8, 4, 4, 4, 12];
    let groups: Vec<&str> = lower.split('-').collect();
    groups.len() == shapes.len()
        && groups
            .iter()
            .zip(shapes)
            .all(|(group, len)| group.len() == len && group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Resolve `CUDA_VISIBLE_DEVICES` into the ordered list of physical indices
/// it exposes (plans/flare-models.md §3.2).
///
/// - `env` is `None` (variable unset) ⇒ `None` — no resolution applies and
///   `--gpus` indices are physical;
/// - a set-but-unresolvable value — any `GPU-…`/`MIG-…` UUID token, any
///   empty or non-integer token — also ⇒ `None`: the selection is then
///   UNDETECTABLE and the caller must demand an explicit revision.
///
/// All-or-nothing by design: a partially understood list is never trusted
/// for hardware selection.
pub fn resolve_cuda_visible(env: Option<&str>) -> Option<Vec<usize>> {
    let env = env?;
    let mut out = Vec::new();
    for token in env.split(',') {
        let idx = token.trim().parse::<usize>().ok()?;
        out.push(idx);
    }
    Some(out)
}

/// One queried GPU, for error reporting (plans/flare-models.md §3.2).
/// Structured and REPO-AGNOSTIC: nothing here knows about revisions — the
/// CLI layer appends revision guidance from `/refs` itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuId {
    /// PCI device id (the 16-bit device half, vendor `0x10DE` stripped).
    pub device_id: u16,
    /// GPU name as reported by `nvidia-smi`.
    pub name: String,
}

impl fmt::Display for GpuId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (device 0x{:04X})", self.name, self.device_id)
    }
}

/// Structured GPU-detection failure (plans/flare-models.md §3.2).
/// Repo-agnostic by contract: [`fmt::Display`] never mentions revisions;
/// the CLI layer owns the "pass an explicit revision" guidance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuDetectError {
    /// `nvidia-smi` is missing or could not be launched (no NVIDIA driver).
    NoNvidiaSmi,
    /// `nvidia-smi` ran but its output could not be turned into a decision
    /// (non-zero exit, 5-second timeout, empty output, an unparsable field
    /// on a SELECTED GPU, or a `CUDA_VISIBLE_DEVICES` value that is not
    /// integer-resolvable).
    Unparseable(String),
    /// The queried device id(s) map to no FLARE platform (flare's rule:
    /// never guess — e.g. GB203/GB205 hardware).
    UnknownIds(Vec<GpuId>),
    /// The selected GPUs span more than one platform.
    Mixed(Vec<String>),
    /// A selected index exceeds the device count it was resolved against.
    IndexOutOfRange {
        /// The offending index (physical, or CVD-relative when
        /// `CUDA_VISIBLE_DEVICES` is set).
        selected: usize,
        /// The number of devices the selection was resolved against.
        count: usize,
    },
}

impl fmt::Display for GpuDetectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuDetectError::NoNvidiaSmi => {
                write!(
                    f,
                    "nvidia-smi not found or failed to launch — automatic GPU detection unavailable"
                )
            }
            GpuDetectError::Unparseable(why) => {
                write!(f, "could not interpret GPU detection output: {why}")
            }
            GpuDetectError::UnknownIds(ids) => {
                let listed: Vec<String> = ids.iter().map(GpuId::to_string).collect();
                write!(
                    f,
                    "unrecognized NVIDIA device id(s) with no FLARE platform mapping: {}",
                    listed.join(", ")
                )
            }
            GpuDetectError::Mixed(names) => {
                write!(
                    f,
                    "selected GPUs span multiple FLARE platforms: {}",
                    names.join(", ")
                )
            }
            GpuDetectError::IndexOutOfRange { selected, count } => {
                write!(
                    f,
                    "selected GPU index {selected} is out of range ({count} device(s) visible)"
                )
            }
        }
    }
}

impl std::error::Error for GpuDetectError {}

/// One parsed `nvidia-smi --query-gpu=…` row. `device_id: None` marks a row
/// whose `pci.device_id` field failed to parse; the decision reports it only
/// if that GPU is actually selected (drivers vary in what they render for
/// unselected GPUs — such rows must not break detection).
#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryRow {
    index: usize,
    device_id: Option<u16>,
    name: String,
}

/// Detect the FLARE platform of the selected GPUs — one
/// `nvidia-smi --query-gpu=index,pci.device_id,compute_cap,name
/// --format=csv,noheader` subprocess with a 5-second budget
/// (plans/flare-models.md §3.2); sync by contract — async callers
/// wrap this in `tokio::task::spawn_blocking`.
///
/// When `CUDA_VISIBLE_DEVICES` is set to integer indices, the selected
/// indices are resolved through it first (flare's semantics); a
/// set-but-unresolvable value fails detection, and an index beyond the
/// visible list fails with [`GpuDetectError::IndexOutOfRange`] before any
/// subprocess is spawned.
pub fn detect_platform(selected: &[usize]) -> Result<FlarePlatform, GpuDetectError> {
    if selected.is_empty() {
        return Err(GpuDetectError::Unparseable(
            "empty GPU selection".to_string(),
        ));
    }
    // Resolve CVD-relative indices first: a selection that cannot name
    // existing devices needs no subprocess to be rejected.
    let resolved: Vec<usize> = match std::env::var("CUDA_VISIBLE_DEVICES").ok().as_deref() {
        None => selected.to_vec(),
        Some(env) => {
            let Some(visible) = resolve_cuda_visible(Some(env)) else {
                return Err(GpuDetectError::Unparseable(
                    "CUDA_VISIBLE_DEVICES is set but not integer-resolvable (GPU/MIG UUID or malformed entries)"
                        .to_string(),
                ));
            };
            let mut mapped = Vec::with_capacity(selected.len());
            for &index in selected {
                let Some(&physical) = visible.get(index) else {
                    return Err(GpuDetectError::IndexOutOfRange {
                        selected: index,
                        count: visible.len(),
                    });
                };
                mapped.push(physical);
            }
            mapped
        }
    };
    let output = run_nvidia_smi()?;
    platform_from_query_rows(&resolved, &output)
}

/// Thin subprocess shell: one `nvidia-smi` query, [`NVIDIA_SMI_TIMEOUT`]
/// budget. Spawn failure ⇒ [`GpuDetectError::NoNvidiaSmi`]; anything else
/// that prevents a decision (non-zero exit, timeout, empty output) ⇒
/// [`GpuDetectError::Unparseable`]. stdout is drained on a helper thread so
/// the poll loop cannot deadlock on a full pipe buffer.
fn run_nvidia_smi() -> Result<String, GpuDetectError> {
    let mut child = Command::new("nvidia-smi")
        .arg("--query-gpu=index,pci.device_id,compute_cap,name")
        .arg("--format=csv,noheader")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| GpuDetectError::NoNvidiaSmi)?;
    let mut stdout = child.stdout.take().expect("stdout piped above");
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= NVIDIA_SMI_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Killing the child closes the pipe, so the reader
                    // thread returns whatever was written before the
                    // timeout instead of blocking forever.
                    let _ = reader.join();
                    return Err(GpuDetectError::Unparseable(format!(
                        "nvidia-smi timed out after {}s",
                        NVIDIA_SMI_TIMEOUT.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = reader.join();
                return Err(GpuDetectError::Unparseable(format!(
                    "failed to wait on nvidia-smi: {e}"
                )));
            }
        }
    };
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(GpuDetectError::Unparseable(format!(
            "nvidia-smi exited with {status}"
        )));
    }
    if output.trim().is_empty() {
        return Err(GpuDetectError::Unparseable(
            "nvidia-smi produced no output".to_string(),
        ));
    }
    Ok(output)
}

/// The whole decision logic of [`detect_platform`], pure over the
/// `nvidia-smi` CSV stdout — unit-tested from inline fixtures with no
/// subprocess (plans/flare-models.md §3.2).
///
/// `selected` holds ALREADY-RESOLVED physical indices: the
/// `CUDA_VISIBLE_DEVICES` mapping happens in [`detect_platform`], which owns
/// the environment. Check order (deterministic, documented): a malformed
/// line or an unparsable `pci.device_id` on a SELECTED GPU short-circuits
/// in selection order (`IndexOutOfRange` / `Unparseable`); then unknown
/// device ids ([`GpuDetectError::UnknownIds`]) win over mixed platforms;
/// then a selection spanning both platforms ⇒
/// [`GpuDetectError::Mixed`] carrying all selected GPU names.
fn platform_from_query_rows(
    selected: &[usize],
    output: &str,
) -> Result<FlarePlatform, GpuDetectError> {
    if selected.is_empty() {
        return Err(GpuDetectError::Unparseable(
            "empty GPU selection".to_string(),
        ));
    }
    let rows = parse_query_rows(output);
    let mut unknowns: Vec<GpuId> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut platform: Option<FlarePlatform> = None;
    let mut mixed = false;
    for &index in selected {
        let Some(row) = rows.iter().find(|row| row.index == index) else {
            return Err(GpuDetectError::IndexOutOfRange {
                selected: index,
                count: rows.len(),
            });
        };
        let Some(device_id) = row.device_id else {
            return Err(GpuDetectError::Unparseable(format!(
                "GPU {index} ({}) reports an unusable pci.device_id field",
                row.name
            )));
        };
        names.push(row.name.clone());
        match platform_from_device_id(device_id) {
            Some(found) => match platform {
                None => platform = Some(found),
                Some(previous) if previous != found => mixed = true,
                _ => {}
            },
            None => {
                let id = GpuId {
                    device_id,
                    name: row.name.clone(),
                };
                if !unknowns.contains(&id) {
                    unknowns.push(id);
                }
            }
        }
    }
    if !unknowns.is_empty() {
        return Err(GpuDetectError::UnknownIds(unknowns));
    }
    if mixed {
        return Err(GpuDetectError::Mixed(names));
    }
    // Unreachable for a non-empty selection of parseable rows; kept for
    // totality.
    platform.ok_or_else(|| GpuDetectError::Unparseable("no platform resolved".to_string()))
}

/// Parse the `--format=csv,noheader` stdout into rows, in file order.
/// Lenient by design (plans/flare-models.md §3.2 risk note: "rendering
/// variance"): a line with fewer than the four queried fields, or whose
/// index field is not an integer, is skipped entirely; a row whose
/// `pci.device_id` field is unusable keeps its index/name with
/// `device_id: None`. A repeated index keeps its FIRST row.
fn parse_query_rows(output: &str) -> Vec<QueryRow> {
    let mut rows: Vec<QueryRow> = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() < 4 {
            continue;
        }
        let Ok(index) = fields[0].trim().parse::<usize>() else {
            continue;
        };
        let device_id = device_id_from_field(fields[1]);
        if rows.iter().any(|row| row.index == index) {
            continue;
        }
        rows.push(QueryRow {
            index,
            device_id,
            name: fields[3].trim().to_string(),
        });
    }
    rows
}

/// Parse one `pci.device_id` CSV field. `nvidia-smi` renders either the
/// 32-bit `0x<device><vendor>` form (`0x2BB910DE`) or the bare 16-bit
/// `0x<device>` form (`0x2BB9`); for the 32-bit form the 16-bit half that
/// is not the NVIDIA vendor id [`NVIDIA_VENDOR_ID`] wins
/// (plans/flare-models.md §3.2). If neither half is the vendor id the
/// device half (upper, PCI convention) wins.
fn device_id_from_field(field: &str) -> Option<u16> {
    let hex = field.trim();
    let hex = hex
        .strip_prefix("0x")
        .or_else(|| hex.strip_prefix("0X"))
        .unwrap_or(hex);
    let value = u32::from_str_radix(hex, 16).ok()?;
    if value > 0xFFFF {
        let high = (value >> 16) as u16;
        let low = value as u16;
        Some(if high != NVIDIA_VENDOR_ID { high } else { low })
    } else {
        Some(value as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- pinned device-id table (plans/flare-models.md §3.2) ----------

    #[test]
    fn platform_table_gb10_entry() {
        assert_eq!(platform_from_device_id(0x2E12), Some(FlarePlatform::Gb10));
    }

    #[test]
    fn platform_table_gb202_entries() {
        for id in [
            0x2B85, 0x2B87, 0x2B8C, 0x2BB1, 0x2BB2, 0x2BB3, 0x2BB4, 0x2BB5, 0x2BB9,
        ] {
            assert_eq!(
                platform_from_device_id(id),
                Some(FlarePlatform::Gb202),
                "device id 0x{id:04X} must map to gb202"
            );
        }
    }

    #[test]
    fn platform_table_pins_gb203_gb205_absent() {
        // GB203 / GB205 must NOT map — shipping them a gb202 package would
        // be a broken install (plans/flare-models.md §6 risk). Unknown ⇒
        // demand --revision.
        assert_eq!(platform_from_device_id(0x2C02), None, "GB203");
        assert_eq!(platform_from_device_id(0x2F04), None, "GB205");
        // Non-device / garbage ids too.
        assert_eq!(platform_from_device_id(0x10DE), None);
        assert_eq!(platform_from_device_id(0x0000), None);
    }

    #[test]
    fn platform_as_str_matches_flare_tokens() {
        assert_eq!(FlarePlatform::Gb202.as_str(), "gb202");
        assert_eq!(FlarePlatform::Gb10.as_str(), "gb10");
    }

    // ---------- both pci.device_id renderings ----------

    #[test]
    fn device_id_field_both_renderings() {
        // 32-bit `0x<device><vendor>` form: the ≠0x10DE half wins.
        assert_eq!(device_id_from_field("0x2BB910DE"), Some(0x2BB9));
        assert_eq!(device_id_from_field("0x2E1210DE"), Some(0x2E12));
        // Byte-swapped edge: vendor half on top.
        assert_eq!(device_id_from_field("0x10DE2BB9"), Some(0x2BB9));
        // Nonstandard neither-half-vendor: upper (device) half wins.
        assert_eq!(device_id_from_field("0x2BB92BB9"), Some(0x2BB9));
        // Bare 16-bit rendering.
        assert_eq!(device_id_from_field("0x2BB9"), Some(0x2BB9));
        assert_eq!(device_id_from_field("0x2E12"), Some(0x2E12));
        // Whitespace and case tolerance.
        assert_eq!(device_id_from_field(" 0x2bb910de "), Some(0x2BB9));
        // Bare vendor id maps to no platform downstream.
        assert_eq!(device_id_from_field("0x10DE"), Some(0x10DE));
        // Unusable fields.
        assert_eq!(device_id_from_field("N/A"), None);
        assert_eq!(device_id_from_field(""), None);
        assert_eq!(device_id_from_field("0xZZZZ"), None);
    }

    // ---------- parse_gpus_spec ----------

    #[test]
    fn gpus_spec_ints_ranges_and_order() {
        assert_eq!(parse_gpus_spec("0"), Ok(vec![0]));
        assert_eq!(parse_gpus_spec("0,1,2"), Ok(vec![0, 1, 2]));
        assert_eq!(parse_gpus_spec("0-3"), Ok(vec![0, 1, 2, 3]));
        // Insertion order preserved across mixed tokens.
        assert_eq!(parse_gpus_spec("2,0-1"), Ok(vec![2, 0, 1]));
        assert_eq!(parse_gpus_spec(" 1 , 3 "), Ok(vec![1, 3]));
        assert_eq!(parse_gpus_spec("0 - 2"), Ok(vec![0, 1, 2]));
    }

    #[test]
    fn gpus_spec_duplicates_collapse_silently() {
        assert_eq!(parse_gpus_spec("1,1,1"), Ok(vec![1]));
        assert_eq!(parse_gpus_spec("0,0-1,1"), Ok(vec![0, 1]));
        assert_eq!(parse_gpus_spec("1-3,2"), Ok(vec![1, 2, 3]));
    }

    #[test]
    fn gpus_spec_empty_is_error() {
        assert!(parse_gpus_spec("").is_err());
        assert!(parse_gpus_spec("   ").is_err());
    }

    #[test]
    fn gpus_spec_inverted_range_is_error() {
        let err = parse_gpus_spec("5-2").unwrap_err();
        assert!(err.contains("exceeds"), "unexpected error: {err}");
        // Inversion inside a longer spec fails the whole spec.
        assert!(parse_gpus_spec("0,5-2").is_err());
    }

    #[test]
    fn gpus_spec_malformed_tokens_are_errors() {
        assert!(parse_gpus_spec("0,,1").is_err());
        assert!(parse_gpus_spec("abc").is_err());
        assert!(parse_gpus_spec("0,abc").is_err());
        assert!(parse_gpus_spec("-1").is_err());
        assert!(parse_gpus_spec("1-2-3").is_err());
    }

    #[test]
    fn gpus_spec_uuid_tokens_point_at_revision() {
        for token in [
            "GPU-1a2b3c4d-1e2f-3a4b-5c6d-7e8f9a0b1c2d",
            "mig-gpu-1a2b3c4d-1e2f-3a4b-5c6d-7e8f9a0b1c2d",
            "12345678-1234-1234-1234-123456789abc",
        ] {
            let err = parse_gpus_spec(&format!("0,{token}")).unwrap_err();
            assert!(
                err.to_lowercase().contains("revision"),
                "token {token} must point at --revision, got: {err}"
            );
        }
    }

    // ---------- resolve_cuda_visible ----------

    #[test]
    fn cuda_visible_unset_is_none() {
        assert_eq!(resolve_cuda_visible(None), None);
    }

    #[test]
    fn cuda_visible_integer_lists_resolve_in_order() {
        assert_eq!(resolve_cuda_visible(Some("0")), Some(vec![0]));
        assert_eq!(resolve_cuda_visible(Some("1,3")), Some(vec![1, 3]));
        assert_eq!(resolve_cuda_visible(Some(" 0 , 2 ")), Some(vec![0, 2]));
    }

    #[test]
    fn cuda_visible_uuid_mig_empty_tokens_are_none() {
        assert_eq!(resolve_cuda_visible(Some("")), None);
        assert_eq!(resolve_cuda_visible(Some("   ")), None);
        assert_eq!(
            resolve_cuda_visible(Some("GPU-1a2b3c4d-1e2f-3a4b-5c6d-7e8f9a0b1c2d")),
            None
        );
        assert_eq!(
            resolve_cuda_visible(Some("MIG-GPU-1a2b3c4d-1e2f-3a4b-5c6d-7e8f9a0b1c2d")),
            None
        );
        // All-or-nothing: one bad token rejects the whole list.
        assert_eq!(resolve_cuda_visible(Some("0,GPU-abc")), None);
        assert_eq!(resolve_cuda_visible(Some("0,,1")), None);
        assert_eq!(resolve_cuda_visible(Some("0,abc")), None);
        assert_eq!(resolve_cuda_visible(Some("-1")), None);
    }

    // ---------- platform_from_query_rows (inline CSV fixtures, no subprocess) ----------

    #[test]
    fn query_rows_single_gpu_both_renderings() {
        let csv32 = "0, 0x2BB910DE, 12.0, NVIDIA GeForce RTX 5090\n";
        assert_eq!(
            platform_from_query_rows(&[0], csv32),
            Ok(FlarePlatform::Gb202)
        );
        let csv_bare = "0, 0x2BB9, 12.0, NVIDIA GeForce RTX 5090\n";
        assert_eq!(
            platform_from_query_rows(&[0], csv_bare),
            Ok(FlarePlatform::Gb202)
        );
        let csv_gb10 = "0, 0x2E1210DE, 12.0, NVIDIA GB10\n";
        assert_eq!(
            platform_from_query_rows(&[0], csv_gb10),
            Ok(FlarePlatform::Gb10)
        );
    }

    #[test]
    fn query_rows_multi_gpu_selection() {
        let csv = "0, 0x2BB910DE, 12.0, NVIDIA RTX 5880 Ada\n\
                   1, 0x2BB910DE, 12.0, NVIDIA RTX 5880 Ada\n\
                   2, 0x2E1210DE, 12.0, NVIDIA GB10\n";
        assert_eq!(
            platform_from_query_rows(&[0, 1], csv),
            Ok(FlarePlatform::Gb202)
        );
        assert_eq!(platform_from_query_rows(&[2], csv), Ok(FlarePlatform::Gb10));
        // Mixed platforms carry ALL selected GPU names, in selection order.
        assert_eq!(
            platform_from_query_rows(&[0, 2], csv),
            Err(GpuDetectError::Mixed(vec![
                "NVIDIA RTX 5880 Ada".to_string(),
                "NVIDIA GB10".to_string()
            ]))
        );
    }

    #[test]
    fn query_rows_unknown_ids_carry_name_and_device() {
        let csv = "0, 0x2C0210DE, 12.0, NVIDIA GB203\n\
                   1, 0x2BB910DE, 12.0, NVIDIA RTX 5880 Ada\n";
        // Unknown beats a known sibling.
        assert_eq!(
            platform_from_query_rows(&[0], csv),
            Err(GpuDetectError::UnknownIds(vec![GpuId {
                device_id: 0x2C02,
                name: "NVIDIA GB203".to_string()
            }]))
        );
        let err = platform_from_query_rows(&[0, 1], csv).unwrap_err();
        assert!(matches!(err, GpuDetectError::UnknownIds(_)));
        // Repo-agnostic contract: no variant's Display mentions revisions.
        assert!(!err.to_string().to_lowercase().contains("revision"));
    }

    #[test]
    fn query_rows_index_out_of_range() {
        let csv = "0, 0x2BB910DE, 12.0, NVIDIA RTX 5880 Ada\n";
        assert_eq!(
            platform_from_query_rows(&[0, 1], csv),
            Err(GpuDetectError::IndexOutOfRange {
                selected: 1,
                count: 1
            })
        );
        // Unparsable output behaves as zero devices.
        assert_eq!(
            platform_from_query_rows(&[0], "garbage line\n"),
            Err(GpuDetectError::IndexOutOfRange {
                selected: 0,
                count: 0
            })
        );
    }

    #[test]
    fn query_rows_empty_selection_is_error() {
        let csv = "0, 0x2BB910DE, 12.0, NVIDIA RTX 5880 Ada\n";
        assert!(matches!(
            platform_from_query_rows(&[], csv),
            Err(GpuDetectError::Unparseable(_))
        ));
    }

    #[test]
    fn query_rows_lenient_toward_unselected_garbage() {
        // An unusable device-id field on an UNSELECTED GPU must not fail
        // detection; the same field on a SELECTED GPU must fail it.
        let csv = "0, N/A, 12.0, Mystery GPU\n1, 0x2BB910DE, 12.0, NVIDIA RTX 5880 Ada\n";
        assert_eq!(
            platform_from_query_rows(&[1], csv),
            Ok(FlarePlatform::Gb202)
        );
        assert!(matches!(
            platform_from_query_rows(&[0], csv),
            Err(GpuDetectError::Unparseable(_))
        ));
        // A malformed line (fewer than 4 fields) is skipped entirely.
        let csv = "nvidia-smi: note\n0, 0x2E1210DE, 12.0, NVIDIA GB10\n";
        assert_eq!(platform_from_query_rows(&[0], csv), Ok(FlarePlatform::Gb10));
    }

    #[test]
    fn query_rows_duplicate_index_keeps_first_row() {
        let csv = "0, 0x2E1210DE, 12.0, NVIDIA GB10\n\
                   0, 0x2BB910DE, 12.0, NVIDIA RTX 5880 Ada\n";
        assert_eq!(platform_from_query_rows(&[0], csv), Ok(FlarePlatform::Gb10));
    }

    #[test]
    fn error_display_never_mentions_revisions() {
        // The repo-agnostic contract (plans/flare-models.md §3.2): the CLI
        // layer appends revision guidance; detection never does.
        let rendered = vec![
            GpuDetectError::NoNvidiaSmi.to_string(),
            GpuDetectError::Unparseable("boom".to_string()).to_string(),
            GpuDetectError::UnknownIds(vec![GpuId {
                device_id: 0x2C02,
                name: "NVIDIA GB203".to_string(),
            }])
            .to_string(),
            GpuDetectError::Mixed(vec!["GPU A".to_string(), "GPU B".to_string()]).to_string(),
            GpuDetectError::IndexOutOfRange {
                selected: 4,
                count: 2,
            }
            .to_string(),
        ];
        for message in rendered {
            assert!(
                !message.to_lowercase().contains("revision"),
                "repo-agnostic leak: {message}"
            );
        }
    }
}
