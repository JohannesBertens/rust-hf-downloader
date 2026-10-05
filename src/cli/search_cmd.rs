//! Search subcommand: query-only, one bounded API call, no engine.

use std::io::Write;

use super::args::ModelDto;
use super::args::{merge_token, SearchArgs};
use super::events::Event;
use super::report::truncate_path;
use super::report::Reporter;
use super::{ProgressMode, EXIT_FAILURE, EXIT_OK};

/// Effective search parameters: explicit flag → config default (the same
/// defaults the TUI's filter toolbar starts with).
pub(super) fn effective_search_params(
    args: &SearchArgs,
    options: &crate::models::AppOptions,
) -> (
    crate::models::SortField,
    crate::models::SortDirection,
    u64,
    u64,
) {
    (
        args.sort
            .map(Into::into)
            .unwrap_or(options.default_sort_field),
        args.direction
            .map(Into::into)
            .unwrap_or(options.default_sort_direction),
        args.min_downloads.unwrap_or(options.default_min_downloads),
        args.min_likes.unwrap_or(options.default_min_likes),
    )
}

/// Fixed-column human table on stdout; the result count goes to stderr so
/// the table stays pipeable.
fn render_search_table(models: &[ModelDto]) {
    use std::io::Write;
    let id_width = models
        .iter()
        .map(|m| m.id.chars().count())
        .chain(std::iter::once("MODEL ID".len()))
        .max()
        .unwrap()
        .min(48);

    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(
        stdout,
        "{:<id_w$}  {:>10}  {:>7}  UPDATED",
        "MODEL ID",
        "DOWNLOADS",
        "LIKES",
        id_w = id_width
    );
    let _ = writeln!(stdout, "{}", "-".repeat(id_width + 31));
    for m in models {
        let updated = m
            .last_modified
            .as_deref()
            .and_then(|s| s.split('T').next())
            .unwrap_or("-");
        let _ = writeln!(
            stdout,
            "{:<id_w$}  {:>10}  {:>7}  {}",
            truncate_path(&m.id, id_width),
            crate::utils::format_number(m.downloads),
            crate::utils::format_number(m.likes),
            updated,
            id_w = id_width
        );
    }
    let _ = stdout.flush();
    eprintln!("{} model(s)", models.len());
}

pub(super) async fn run_search(args: SearchArgs) -> i32 {
    let mut reporter = Reporter::new(args.json, false, ProgressMode::Auto);
    let options = crate::config::load_config();
    let token = merge_token(
        args.token.clone(),
        std::env::var("HF_TOKEN").ok(),
        options.hf_token.clone(),
    );

    let (sort, direction, min_downloads, min_likes) = effective_search_params(&args, &options);

    match crate::api::fetch_models_filtered(
        &args.query,
        sort,
        direction,
        min_downloads,
        min_likes,
        args.limit,
        token.as_ref(),
    )
    .await
    {
        Ok(models) => {
            let dtos: Vec<ModelDto> = models.iter().map(ModelDto::from).collect();
            if args.json {
                // Queries emit one JSON document (an array), not NDJSON
                // events — events are for streaming pipelines. On failure the
                // only stdout output is a single error event (see below).
                let mut stdout = std::io::stdout().lock();
                match serde_json::to_string_pretty(&dtos) {
                    Ok(json) => {
                        let _ = writeln!(stdout, "{}", json);
                        let _ = stdout.flush();
                    }
                    Err(e) => {
                        drop(stdout);
                        reporter.emit(&Event::Error {
                            code: "internal".to_string(),
                            message: format!("failed to serialize results: {}", e),
                            available: None,
                        });
                        return EXIT_FAILURE;
                    }
                }
            } else if dtos.is_empty() {
                // A successful query with zero hits is still success (exit 0);
                // scripts distinguish via the empty array / table absence.
                eprintln!("No models found.");
            } else {
                render_search_table(&dtos);
            }
            EXIT_OK
        }
        Err(e) => {
            reporter.emit(&Event::Error {
                code: "network".to_string(),
                message: format!("search failed: {}", e),
                available: None,
            });
            EXIT_FAILURE
        }
    }
}
