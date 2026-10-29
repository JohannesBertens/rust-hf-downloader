//! FLARE display mode (plan `plans/flare-models.md` §3.4): the
//! revision-cards panel (left) and the repo-facts panel (right), the
//! `gguf.rs` precedent. Pure render pass — everything arrives through
//! [`FlarePanelContext`]; hit-rects are registered into the caller's
//! `panel_areas` (Results list first, then this pane — registration order
//! is behavior, see `render::MouseAreas`).
//!
//! The right panel carries no focusable pane (read-only facts, like
//! Standard mode's metadata panel) and therefore registers no hit-rect.

use super::{border_style, panel_list, FocusCtx};
use crate::fmt::size_full;
use crate::gpu::FlarePlatform;
use crate::models::{
    FlarePackage, FlareRepoInfo, FlareRevision, FocusedPane, ParsedPackageTokens, RefKind, RepoFile,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

/// FLARE-mode panel inputs (the W5.2 context-group precedent): the
/// per-model scan result, the revision-cards cursor, and the
/// SESSION-scoped platform (plans/flare-models.md §3.4 — detected once,
/// never cleared) that
/// stars the matching cards. `loading` is the shared quant-loading flag.
pub struct FlarePanelContext<'a> {
    pub flare_info: &'a Option<FlareRepoInfo>,
    pub flare_list_state: &'a mut ListState,
    pub flare_platform: &'a Option<FlarePlatform>,
    pub loading: bool,
}

/// The package's files in download order: the packed file, or the
/// directory form's primary + companions (`weights/**` +
/// `deployments/**`). Shared with the TUI download flow
/// (plans/flare-models.md §3.5) so the
/// cards' sizes and the queued set can never disagree.
pub fn flare_package_files(package: &FlarePackage) -> Vec<&RepoFile> {
    match package {
        FlarePackage::Packed { file } => vec![file],
        FlarePackage::Directory {
            primary,
            companions,
        } => std::iter::once(primary).chain(companions.iter()).collect(),
    }
}

/// Total package bytes; a file with no known size counts 0 (the card
/// stays honest about unknowns instead of hiding the file).
pub fn flare_package_size(package: &FlarePackage) -> u64 {
    flare_package_files(package)
        .iter()
        .map(|file| file.size.unwrap_or(0))
        .sum()
}

/// The primary package filename — the name the token grammar parsed.
pub fn flare_package_name(package: &FlarePackage) -> &str {
    match package {
        FlarePackage::Packed { file } => &file.rfilename,
        FlarePackage::Directory { primary, .. } => &primary.rfilename,
    }
}

/// `★ ` when the session-detected platform matches the card's parsed arch
/// token (both flare-canonical lowercase, e.g. `gb202`); empty otherwise.
fn platform_star(arch: Option<&str>, platform: Option<&FlarePlatform>) -> &'static str {
    match (arch, platform) {
        (Some(arch), Some(platform)) if arch == platform.as_str() => "★ ",
        _ => "",
    }
}

/// The kind badge of a refs entry (`branch` / `tag`).
fn kind_badge(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Branch => "branch",
        RefKind::Tag => "tag",
    }
}

/// Short commit — first 8 hex chars, display-only (the full 40-hex pin
/// rides the status line and the registry entries, never this card).
fn short_commit(commit: &str) -> &str {
    &commit[..commit.len().min(8)]
}

/// The card's parsed-token suffix, e.g. ` [tp2 c16 mtp5 i4 yarn4]`
/// (display only — plans/flare-models.md §3.1's grammar, unknown tokens
/// already dropped).
fn parsed_tokens_suffix(parsed: &ParsedPackageTokens) -> String {
    let mut tokens: Vec<String> = Vec::new();
    if let Some(tp) = parsed.tp {
        tokens.push(format!("tp{tp}"));
    }
    if let Some(c) = parsed.max_concurrency {
        tokens.push(format!("c{c}"));
    }
    if let Some(mtp) = parsed.mtp_draft_tokens {
        tokens.push(format!("mtp{mtp}"));
    }
    if parsed.quant_i4 {
        tokens.push("i4".to_string());
    }
    tokens.extend(parsed.variants.iter().cloned());
    if tokens.is_empty() {
        String::new()
    } else {
        format!(" [{}]", tokens.join(" "))
    }
}

/// One revision card: two lines — the head (star, revision + aliases,
/// kind badge, short commit) and the tail (size in `fmt.rs` FULL form,
/// package name, parsed tokens, file count for directory packages).
fn revision_card(revision: &FlareRevision, platform: Option<&FlarePlatform>) -> ListItem<'static> {
    let star = platform_star(revision.parsed.arch.as_deref(), platform);
    let aliases = if revision.aliases.is_empty() {
        String::new()
    } else {
        format!(" ({})", revision.aliases.join(", "))
    };
    let head = Line::from(vec![
        Span::styled(
            format!("{star}{}", revision.revision),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(aliases, Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled(
            format!("[{}]", kind_badge(revision.kind)),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            format!(" @{}", short_commit(&revision.target_commit)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let size = flare_package_size(&revision.package);
    let files = flare_package_files(&revision.package).len();
    let file_count = if files > 1 {
        format!(" (+{} files)", files - 1)
    } else {
        String::new()
    };
    let tail = Line::from(vec![
        Span::styled(
            format!("  {:>10} ", size_full(size)),
            Style::default().fg(Color::Green),
        ),
        Span::raw(flare_package_name(&revision.package).to_string()),
        Span::styled(
            format!("{}{}", parsed_tokens_suffix(&revision.parsed), file_count),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    ListItem::new(vec![head, tail])
}

/// The right panel's fact rows for the repo-wide scan facts: the
/// config-facts header (plans/flare-models.md §3.4) and the publisher line
/// with fingerprint.
fn facts_lines(flare_info: &FlareRepoInfo) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(Line::from(Span::styled(
        "Config",
        Style::default().fg(Color::Yellow),
    )));
    let facts = &flare_info.config;
    let mut fact_rows = 0;
    for (label, value) in [
        (
            "serving-context",
            facts.serving_context.map(|v| v.to_string()),
        ),
        ("execution-abi", facts.execution_abi.map(|v| v.to_string())),
        (
            "prepared-model-contract",
            facts.prepared_model_contract.map(|v| v.to_string()),
        ),
        ("checkpoint", facts.checkpoint.clone()),
    ] {
        if let Some(value) = value {
            lines.push(Line::from(format!("  {label}: {value}")));
            fact_rows += 1;
        }
    }
    if fact_rows == 0 {
        lines.push(Line::from(Span::styled(
            "  (none reported)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Publisher",
        Style::default().fg(Color::Yellow),
    )));
    match &flare_info.publisher {
        Some(publisher) => {
            let identity = match (&publisher.namespace, &publisher.name) {
                (Some(namespace), Some(name)) => format!("{namespace}/{name}"),
                (Some(namespace), None) => namespace.clone(),
                (None, Some(name)) => name.clone(),
                (None, None) => "(unnamed)".to_string(),
            };
            let development = if publisher.development == Some(true) {
                " [development]"
            } else {
                ""
            };
            lines.push(Line::from(format!("  {identity}{development}")));
            if let Some(fingerprint) = &publisher.fingerprint {
                // Display-only claim (D7): sha256-hex over the raw public
                // key. Shortened to 16 chars — the full digest never fits
                // a half-width panel.
                let short = &fingerprint[..fingerprint.len().min(16)];
                lines.push(Line::from(Span::styled(
                    format!("  fingerprint: {short}…"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "  (no publisher.json identity)",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    if !flare_info.dropped_refs.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "dropped refs (no commit pin): {}",
                flare_info.dropped_refs.join(", ")
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
}

pub(super) fn render_flare_panels(
    frame: &mut Frame,
    chunks: std::rc::Rc<[Rect]>,
    ctx: FlarePanelContext<'_>,
    focus: &FocusCtx,
    panel_areas: &mut Vec<(FocusedPane, Rect)>,
) {
    let FlarePanelContext {
        flare_info,
        flare_list_state,
        flare_platform,
        loading,
    } = ctx;

    // Left side: revision cards
    let cards_title = if loading {
        "FLARE Revisions [Loading...]"
    } else if flare_info.is_none() {
        "FLARE Revisions [Select a model to view]"
    } else {
        "FLARE Revisions"
    };

    let items: Vec<ListItem> = flare_info
        .iter()
        .flat_map(|info| info.revisions.iter())
        .map(|revision| revision_card(revision, flare_platform.as_ref()))
        .collect();

    let cards = panel_list(
        items,
        cards_title,
        border_style(FocusedPane::FlareRevisions, focus),
    );

    // Register panel area for click/hover detection (after the Results
    // list — registration order is behavior, see `MouseAreas`)
    panel_areas.push((FocusedPane::FlareRevisions, chunks[0]));
    frame.render_stateful_widget(cards, chunks[0], flare_list_state);

    // Right side: repo-wide facts (config header + publisher). Read-only,
    // no focusable pane, no hit-rect — the ModelMetadata precedent.
    let facts_content = match flare_info {
        Some(info) => facts_lines(info),
        None => vec![Line::from(Span::styled(
            "No FLARE scan for this model",
            Style::default().fg(Color::DarkGray),
        ))],
    };
    let facts = Paragraph::new(facts_content)
        .block(Block::default().borders(Borders::ALL).title("FLARE Info"))
        .wrap(Wrap { trim: false });
    frame.render_widget(facts, chunks[1]);
}
