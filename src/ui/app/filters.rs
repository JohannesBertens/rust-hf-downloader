//! Filter & sort state (W4.5): the four filter values plus the cycling/
//! stepping rules that mutate them, single-homed here after being copied
//! across nine call sites (keyboard 's'/'S'/'r'/'+/-' in `events.rs`,
//! click + scroll in `app.rs`, presets).
//!
//! ## Divergence table (built BEFORE unifying — see the W4.5 commit)
//!
//! Two intentional per-site semantics exist and are kept as DISTINCT
//! methods; they must not be silently unified:
//!
//! | Mutation | Sites | Semantics |
//! |---|---|---|
//! | sort field cycle | mouse click, mouse scroll, 's' key | wrap-around forward/backward (`cycle(0, _)`) |
//! | sort field '+' | keyboard `modify_focused_filter(+1)` | same forward cycle (`step(0, +1)` delegates) |
//! | sort field '−' | keyboard `modify_focused_filter(−1)` | toggles sort DIRECTION instead of cycling (`step(0, −1)`) — NOT the backward cycle |
//! | sort direction | 'S' key | toggle (`toggle_direction`) |
//! | min downloads / likes, mouse | click, scroll | wrap-around over the step table (`cycle(1/2, _)`) |
//! | min downloads / likes, keyboard | '+/-' | CLAMPED at both table ends (`step(1/2, _)`) |
//!
//! Values outside a step table (e.g. `default_min_downloads = 42` loaded
//! from config) resolve to table index 0 at every historical site; both
//! methods preserve that. Status messages and their write ORDER stay at
//! the App call sites (the mouse sites write the new-value status BEFORE
//! the refresh tail overwrites it with "Searching...", while the 's'/'S'/
//! 'r' keys write AFTER — observable, preserved by keeping the tails at
//! the call sites, gated on [`FilterState::refresh_request`]).

use crate::models::{AppOptions, FilterPreset, SortDirection, SortField};

/// Step table for the min-downloads filter (click/scroll/keyboard share it).
pub(crate) const DOWNLOAD_STEPS: [u64; 6] = [0, 100, 1_000, 10_000, 100_000, 1_000_000];
/// Step table for the min-likes filter (click/scroll/keyboard share it).
pub(crate) const LIKE_STEPS: [u64; 7] = [0, 10, 50, 100, 500, 1_000, 5_000];

/// Filter & sort values. Seeded from `AppOptions` defaults on startup and
/// written back to them by `App::save_filter_settings`; the config
/// load/save mapping itself lives in `AppOptions` (serde) and is unchanged.
#[derive(Debug)]
pub struct FilterState {
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub min_downloads: u64,
    pub min_likes: u64,
    /// Set by every mutation below; consumed by [`Self::refresh_request`].
    needs_refresh: bool,
}

impl FilterState {
    /// Seed from the persisted config defaults (`AppOptions::default_*`).
    pub fn from_options(options: &AppOptions) -> Self {
        Self {
            sort_field: options.default_sort_field,
            sort_direction: options.default_sort_direction,
            min_downloads: options.default_min_downloads,
            min_likes: options.default_min_likes,
            needs_refresh: false,
        }
    }

    /// Mouse click/scroll semantics: wrap-around in `forward` (click,
    /// scroll-down, 's') or backward (scroll-up) direction. `field` is the
    /// toolbar field index: 0 = sort, 1 = min downloads, 2 = min likes.
    /// Unknown indices are a no-op on the values but still mark a refresh,
    /// matching the historical unconditional refresh tail.
    pub fn cycle(&mut self, field: usize, forward: bool) {
        match field {
            0 => self.sort_field = cycle_sort_field(self.sort_field, forward),
            1 => {
                self.min_downloads = step_table(&DOWNLOAD_STEPS, self.min_downloads, wrap(forward))
            }
            2 => self.min_likes = step_table(&LIKE_STEPS, self.min_likes, wrap(forward)),
            _ => {}
        }
        self.needs_refresh = true;
    }

    /// Keyboard '+/-' semantics (`modify_focused_filter`): table steps
    /// CLAMP at both ends (no wrap), and field 0 intentionally diverges —
    /// '+' cycles the sort field forward while '−' toggles the sort
    /// direction (historical keyboard behavior, not the backward cycle).
    pub fn step(&mut self, field: usize, delta: i32) {
        match field {
            0 => {
                if delta > 0 {
                    self.sort_field = cycle_sort_field(self.sort_field, true);
                } else {
                    self.toggle_direction();
                }
            }
            1 => self.min_downloads = step_table(&DOWNLOAD_STEPS, self.min_downloads, clamp(delta)),
            2 => self.min_likes = step_table(&LIKE_STEPS, self.min_likes, clamp(delta)),
            _ => {}
        }
        self.needs_refresh = true;
    }

    /// Toggle ascending/descending ('S' key and `step(0, −1)`).
    pub fn toggle_direction(&mut self) {
        self.sort_direction = match self.sort_direction {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        };
        self.needs_refresh = true;
    }

    /// Reset to the compiled-in defaults ('r' key).
    pub fn reset(&mut self) {
        self.sort_field = SortField::default();
        self.sort_direction = SortDirection::default();
        self.min_downloads = 0;
        self.min_likes = 0;
        self.needs_refresh = true;
    }

    /// Apply a preset's four values ('1'-'4' keys). Status wording stays
    /// at the call site.
    pub fn apply_preset(&mut self, preset: FilterPreset) {
        let (sort_field, sort_direction, min_downloads, min_likes) = preset_values(preset);
        self.sort_field = sort_field;
        self.sort_direction = sort_direction;
        self.min_downloads = min_downloads;
        self.min_likes = min_likes;
        self.needs_refresh = true;
    }

    /// Whether the current values differ from `preset`'s (the
    /// `would_change_settings` guard).
    pub fn matches_preset(&self, preset: FilterPreset) -> bool {
        let (sort_field, sort_direction, min_downloads, min_likes) = preset_values(preset);
        self.sort_field == sort_field
            && self.sort_direction == sort_direction
            && self.min_downloads == min_downloads
            && self.min_likes == min_likes
    }

    /// Consume-and-report: true exactly when a mutation happened since the
    /// last call, gating the `clear_search_results()` +
    /// `needs_search_models = true` tail at each call site.
    pub fn refresh_request(&mut self) -> bool {
        std::mem::replace(&mut self.needs_refresh, false)
    }
}

/// Forward/backward sort-field ring: Downloads → Likes → Modified → Name.
fn cycle_sort_field(field: SortField, forward: bool) -> SortField {
    use SortField::*;
    match (field, forward) {
        (Downloads, true) => Likes,
        (Likes, true) => Modified,
        (Modified, true) => Name,
        (Name, true) => Downloads,
        (Downloads, false) => Name,
        (Likes, false) => Downloads,
        (Modified, false) => Likes,
        (Name, false) => Modified,
    }
}

/// One move over a step table. `next` maps the current table index to the
/// next one; values outside the table resolve to index 0 (the historical
/// `unwrap_or(0)`).
fn step_table(steps: &[u64], current: u64, next: impl Fn(usize, usize) -> usize) -> u64 {
    let idx = steps.iter().position(|&x| x == current).unwrap_or(0);
    steps[next(idx, steps.len())]
}

/// Wrap-around index move (mouse): 0 and the last index wrap into each other.
fn wrap(forward: bool) -> impl Fn(usize, usize) -> usize {
    move |idx, len| {
        if forward {
            (idx + 1) % len
        } else if idx == 0 {
            len - 1
        } else {
            idx - 1
        }
    }
}

/// Clamped index move (keyboard '+/-'): stops at both table ends.
fn clamp(delta: i32) -> impl Fn(usize, usize) -> usize {
    move |idx, len| {
        if delta > 0 {
            (idx + 1).min(len - 1)
        } else {
            idx.saturating_sub(1)
        }
    }
}

/// The single preset table (previously duplicated between
/// `would_change_settings` and `apply_filter_preset`).
fn preset_values(preset: FilterPreset) -> (SortField, SortDirection, u64, u64) {
    match preset {
        FilterPreset::NoFilters => (SortField::Downloads, SortDirection::Descending, 0, 0),
        FilterPreset::Popular => (SortField::Downloads, SortDirection::Descending, 10_000, 100),
        FilterPreset::HighlyRated => (SortField::Likes, SortDirection::Descending, 0, 1_000),
        FilterPreset::Recent => (SortField::Modified, SortDirection::Descending, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> FilterState {
        FilterState {
            sort_field: SortField::Downloads,
            sort_direction: SortDirection::Descending,
            min_downloads: 0,
            min_likes: 0,
            needs_refresh: false,
        }
    }

    // ---- cycle: sort field ring, both directions ----

    #[test]
    fn cycle_sort_field_forward_wraps() {
        let mut f = fresh();
        for expected in [
            SortField::Likes,
            SortField::Modified,
            SortField::Name,
            SortField::Downloads,
        ] {
            f.cycle(0, true);
            assert_eq!(f.sort_field, expected);
        }
    }

    #[test]
    fn cycle_sort_field_backward_wraps() {
        let mut f = fresh();
        for expected in [
            SortField::Name,
            SortField::Modified,
            SortField::Likes,
            SortField::Downloads,
        ] {
            f.cycle(0, false);
            assert_eq!(f.sort_field, expected);
        }
    }

    #[test]
    fn cycle_sort_field_backward_is_inverse_of_forward() {
        let mut f = fresh();
        f.cycle(0, true);
        let forward = f.sort_field;
        f.cycle(0, false);
        assert_eq!(f.sort_field, SortField::Downloads);
        assert_ne!(forward, SortField::Downloads);
    }

    // ---- cycle: downloads table wrap-around ----

    #[test]
    fn cycle_downloads_forward_walks_table_and_wraps() {
        let mut f = fresh();
        let table = [
            100, 1_000, 10_000, 100_000, 1_000_000, 0, // wrap 1M → 0
        ];
        for expected in table {
            f.cycle(1, true);
            assert_eq!(f.min_downloads, expected);
        }
    }

    #[test]
    fn cycle_downloads_backward_wraps_at_zero() {
        let mut f = fresh();
        f.cycle(1, false);
        assert_eq!(f.min_downloads, 1_000_000, "0 must wrap to the last step");
        f.cycle(1, false);
        assert_eq!(f.min_downloads, 100_000);
    }

    // ---- cycle: likes table wrap-around ----

    #[test]
    fn cycle_likes_forward_walks_table_and_wraps() {
        let mut f = fresh();
        let table = [10, 50, 100, 500, 1_000, 5_000, 0];
        for expected in table {
            f.cycle(2, true);
            assert_eq!(f.min_likes, expected);
        }
    }

    #[test]
    fn cycle_likes_backward_wraps_at_zero() {
        let mut f = fresh();
        f.cycle(2, false);
        assert_eq!(f.min_likes, 5_000);
        f.cycle(2, false);
        assert_eq!(f.min_likes, 1_000);
    }

    // ---- off-table values resolve to index 0 (historical unwrap_or(0)) ----

    #[test]
    fn cycle_off_table_value_uses_index_zero() {
        let mut f = fresh();
        f.min_downloads = 42;
        f.cycle(1, true);
        assert_eq!(f.min_downloads, 100, "42 → idx 0 → forward = steps[1]");
        f.min_likes = 7;
        f.cycle(2, false);
        assert_eq!(f.min_likes, 5_000, "7 → idx 0 → backward = last step");
    }

    #[test]
    fn step_off_table_value_uses_index_zero() {
        let mut f = fresh();
        f.min_likes = 7;
        f.step(2, 1);
        assert_eq!(f.min_likes, 10);
        f.min_likes = 7;
        f.step(2, -1);
        assert_eq!(f.min_likes, 0, "idx 0 clamped by saturating_sub");
    }

    // ---- step: keyboard clamp semantics ----

    #[test]
    fn step_downloads_clamps_at_both_ends() {
        let mut f = fresh();
        f.min_downloads = 1_000_000; // last step
        f.step(1, 1);
        assert_eq!(f.min_downloads, 1_000_000, "'+' at max stays (no wrap)");
        f.min_downloads = 0; // first step
        f.step(1, -1);
        assert_eq!(f.min_downloads, 0, "'-' at min stays (no wrap)");
        f.min_downloads = 1_000;
        f.step(1, 1);
        assert_eq!(f.min_downloads, 10_000);
        f.step(1, -1);
        assert_eq!(f.min_downloads, 1_000);
    }

    #[test]
    fn step_likes_clamps_at_both_ends() {
        let mut f = fresh();
        f.min_likes = 5_000;
        f.step(2, 1);
        assert_eq!(f.min_likes, 5_000);
        f.min_likes = 0;
        f.step(2, -1);
        assert_eq!(f.min_likes, 0);
        f.min_likes = 50;
        f.step(2, 1);
        assert_eq!(f.min_likes, 100);
        f.step(2, -1);
        assert_eq!(f.min_likes, 50);
    }

    // ---- step: field 0 divergence ('+' cycles sort, '−' toggles direction) ----

    #[test]
    fn step_sort_plus_cycles_field_forward() {
        let mut f = fresh();
        f.step(0, 1);
        assert_eq!(f.sort_field, SortField::Likes);
        assert_eq!(
            f.sort_direction,
            SortDirection::Descending,
            "no direction change"
        );
    }

    #[test]
    fn step_sort_minus_toggles_direction_not_field() {
        let mut f = fresh();
        f.step(0, -1);
        assert_eq!(f.sort_field, SortField::Downloads, "field must not cycle");
        assert_eq!(f.sort_direction, SortDirection::Ascending);
        f.step(0, -1);
        assert_eq!(f.sort_direction, SortDirection::Descending);
    }

    // ---- toggle_direction / reset ----

    #[test]
    fn toggle_direction_flips_both_ways() {
        let mut f = fresh();
        assert_eq!(f.sort_direction, SortDirection::Descending);
        f.toggle_direction();
        assert_eq!(f.sort_direction, SortDirection::Ascending);
        f.toggle_direction();
        assert_eq!(f.sort_direction, SortDirection::Descending);
    }

    #[test]
    fn reset_restores_compiled_defaults() {
        let mut f = fresh();
        f.cycle(0, true);
        f.toggle_direction();
        f.min_downloads = 10_000;
        f.min_likes = 500;
        f.reset();
        assert_eq!(f.sort_field, SortField::Downloads);
        assert_eq!(f.sort_direction, SortDirection::Descending);
        assert_eq!(f.min_downloads, 0);
        assert_eq!(f.min_likes, 0);
    }

    // ---- presets ----

    #[test]
    fn presets_apply_and_match() {
        for (preset, sort, dir, dl, likes) in [
            (
                FilterPreset::NoFilters,
                SortField::Downloads,
                SortDirection::Descending,
                0,
                0u64,
            ),
            (
                FilterPreset::Popular,
                SortField::Downloads,
                SortDirection::Descending,
                10_000,
                100,
            ),
            (
                FilterPreset::HighlyRated,
                SortField::Likes,
                SortDirection::Descending,
                0,
                1_000,
            ),
            (
                FilterPreset::Recent,
                SortField::Modified,
                SortDirection::Descending,
                0,
                0,
            ),
        ] {
            let mut f = fresh();
            assert!(
                !f.matches_preset(preset) || preset == FilterPreset::NoFilters,
                "fresh state only matches NoFilters"
            );
            f.apply_preset(preset);
            assert_eq!(f.sort_field, sort);
            assert_eq!(f.sort_direction, dir);
            assert_eq!(f.min_downloads, dl);
            assert_eq!(f.min_likes, likes);
            assert!(f.matches_preset(preset));
        }
    }

    // ---- refresh_request consume semantics ----

    #[test]
    fn refresh_request_is_false_when_clean_and_consumed_when_set() {
        let mut f = fresh();
        assert!(!f.refresh_request(), "no mutation yet");
        f.cycle(0, true);
        assert!(f.refresh_request());
        assert!(!f.refresh_request(), "flag must be consumed, not sticky");
        f.step(1, -1);
        f.cycle(2, false);
        f.toggle_direction();
        f.reset();
        f.apply_preset(FilterPreset::Popular);
        assert!(
            f.refresh_request(),
            "one consume per any number of mutations"
        );
    }

    #[test]
    fn unknown_field_still_requests_refresh_like_the_old_unconditional_tail() {
        let mut f = fresh();
        f.cycle(7, true);
        assert_eq!(f.sort_field, SortField::Downloads, "values untouched");
        assert!(
            f.refresh_request(),
            "tail ran unconditionally at every site"
        );
        f.step(9, 1);
        assert!(f.refresh_request());
    }

    // ---- config seeding ----

    #[test]
    fn from_options_seeds_config_defaults() {
        let options = AppOptions {
            default_sort_field: SortField::Name,
            default_sort_direction: SortDirection::Ascending,
            default_min_downloads: 1_000,
            default_min_likes: 50,
            ..AppOptions::default()
        };
        let f = FilterState::from_options(&options);
        assert_eq!(f.sort_field, SortField::Name);
        assert_eq!(f.sort_direction, SortDirection::Ascending);
        assert_eq!(f.min_downloads, 1_000);
        assert_eq!(f.min_likes, 50);
    }
}
