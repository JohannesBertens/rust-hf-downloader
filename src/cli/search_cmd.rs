//! Search subcommand: query-only, one bounded API call, no engine.

use std::io::Write;

use super::args::{ModelDto, SearchArgs};
use super::events::{ErrorCode, Event};
use super::report::{ProgressMode, Reporter};
use super::run::{emit_client_error, resolve_run_token};
use super::{EXIT_FAILURE, EXIT_OK};

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
            crate::fmt::truncate_path_cli(&m.id, id_width),
            crate::fmt::number(m.downloads),
            crate::fmt::number(m.likes),
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
    // Runner partial bootstrap: token by the run precedence (flag >
    // $HF_TOKEN > config); the options stay for the search defaults. The
    // run's one shared client carries the token (M4/B5) — a malformed
    // token stops the search with an explicit auth error instead of a
    // silent unauthenticated query.
    let token = resolve_run_token(args.token.clone(), &options);
    let api_client = match crate::http_client::build_client_with_token(token.as_deref(), None) {
        Ok(client) => client,
        Err(e) => return emit_client_error(&mut reporter, &e),
    };

    let (sort, direction, min_downloads, min_likes) = effective_search_params(&args, &options);

    match crate::api::fetch_models_filtered(
        &api_client,
        &args.query,
        sort,
        direction,
        min_downloads,
        min_likes,
        args.limit,
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
                        reporter.emit(&Event::error(
                            ErrorCode::Internal,
                            format!("failed to serialize results: {}", e),
                        ));
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
            reporter.emit(&Event::error(
                ErrorCode::Network,
                format!("search failed: {}", e),
            ));
            EXIT_FAILURE
        }
    }
}
