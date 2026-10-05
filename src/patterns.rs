//! Python `fnmatch`-compatible glob matching for `hf-cache sync` file
//! selection (plans/hf-cache-sync.md §7).
//!
//! Semantics mirror CPython's `fnmatch` module on a case-sensitive POSIX
//! host — the same matcher `huggingface_hub` applies to its
//! `allow_patterns` — so `--include`/`--exclude` select exactly the files
//! the hub CLI would:
//!
//! - `*` matches any run of characters, **including** `/`: that is why
//!   `*.safetensors` also selects sharded weights nested in subfolders.
//! - `?` matches exactly one character (any character, also `/`).
//! - `[seq]` matches any character in `seq`; `[!seq]` matches any character
//!   not in `seq`. An unclosed `[` is a literal `[`.
//! - There is no `**` special case — it is simply `*` twice.
//! - Everything else, including `/`, is literal. Matching is case-sensitive
//!   and anchored at both ends; patterns apply to the repo-relative POSIX
//!   path (no leading-`/` handling, hub semantics).
//!
//! Patterns are translated to an anchored regex and matched with the
//! `regex` crate. There is no pattern cache: repo file trees are small
//! enough that per-call compilation is fine.

/// Allow-list patterns for `--for vllm` (plans/hf-cache-sync.md §2.3):
/// config/tokenizer/vocab/merges/sentencepiece/chat-template/index files
/// and sharded safetensors in subfolders (`*` crosses `/`). Data table,
/// not code — when vLLM's own `allow_patterns` drift, this is a one-line
/// fix.
#[cfg_attr(not(test), allow(dead_code))] // wired up by the CLI once hf-cache sync lands
pub const VLLM_ALLOW: &[&str] = &[
    "*.safetensors", // sharded weights, incl. subfolders
    "*.json",        // config.json, tokenizer.json, *.safetensors.index.json, …
    "*.txt",         // vocab / merges files
    "*.model",       // sentencepiece models
    "*.jinja",       // chat templates
];

/// Ignore-list patterns for `--for vllm` (plans/hf-cache-sync.md §2.3):
/// the formats vLLM never reads — fallback weight formats, ONNX exports,
/// docs, and the untouched `original/` upstream weights.
#[cfg_attr(not(test), allow(dead_code))] // wired up by the CLI once hf-cache sync lands
pub const VLLM_IGNORE: &[&str] = &[
    "original/**", // redundant-but-readable next to `*` crossing `/` (§7)
    "*.bin",       // pytorch fallback
    "*.pt",
    "*.gguf",
    "*.onnx",
    "onnx/**",
    "*.msgpack",
    "*.h5",
    "*.ot",
    "*.tflite",
    "*.md", // README and friends
    ".gitattributes",
];

/// Test whether `name` matches the shell wildcard `pattern` with CPython
/// `fnmatch` semantics (see the module docs): `*` crosses `/`, `?` is any
/// single character, `[seq]`/`[!seq]` are character classes, everything
/// else is literal; matching is case-sensitive and anchored at both ends.
///
/// Mirrors Python `fnmatch.fnmatchcase`, which is what `fnmatch.fnmatch`
/// reduces to on case-sensitive POSIX systems. A pattern that does not
/// compile into a valid regex (e.g. the reversed range `[z-a]`, which
/// Python rejects at match time with `re.error`) never matches instead of
/// panicking — patterns arrive from `--include`/`--exclude` argv.
#[cfg_attr(not(test), allow(dead_code))] // wired up by the CLI once hf-cache sync lands
pub fn fnmatch(pattern: &str, name: &str) -> bool {
    let anchored = format!(r"(?s)\A{}\z", translate(pattern));
    regex::Regex::new(&anchored).is_ok_and(|re| re.is_match(name))
}

/// Filter `paths` by include/exclude glob lists (§2.2 precedence).
///
/// - An empty `include` list means "everything".
/// - Otherwise a path survives only if it matches **any** include pattern.
/// - `exclude` is applied last: a path matching **any** exclude pattern is
///   dropped regardless of the include list.
///
/// Matching uses [`fnmatch`] over repo-relative POSIX paths; the input
/// order is preserved.
#[cfg_attr(not(test), allow(dead_code))] // wired up by the CLI once hf-cache sync lands
pub fn filter_paths<'a>(paths: &[&'a str], include: &[String], exclude: &[String]) -> Vec<&'a str> {
    paths
        .iter()
        .copied()
        .filter(|path| include.is_empty() || include.iter().any(|pat| fnmatch(pat, path)))
        .filter(|path| !exclude.iter().any(|pat| fnmatch(pat, path)))
        .collect()
}

/// Translate a glob pattern into a regex body, following CPython's
/// `fnmatch.translate`:
///
/// - `*` → `.*` (any characters, including `/`),
/// - `?` → `.`,
/// - `[seq]`/`[!seq]` → character class. The class body is scanned like
///   Python's: a leading `!` and a leading `]` belong to the body, and the
///   class ends at the *first* following `]`. An unclosed `[` is emitted
///   as a literal `\[`.
///
/// Class-body members that the `regex` crate would read differently from
/// Python's `re` are escaped by [`translate_class`] (`[`, `&`, `^`, `]`,
/// `\`): the `regex` crate treats `[[:alpha:]]` as a POSIX class and `&&`
/// as class intersection, while Python `re` — and therefore `fnmatch` —
/// reads them as literal members.
fn translate(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let n = chars.len();
    let mut res = String::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        i += 1;
        match c {
            '*' => res.push_str(".*"),
            '?' => res.push('.'),
            '[' => {
                // Scan for the closing ']' exactly like fnmatch.translate:
                // a leading '!' and a leading ']' are part of the body.
                let mut j = i;
                if j < n && chars[j] == '!' {
                    j += 1;
                }
                if j < n && chars[j] == ']' {
                    j += 1;
                }
                while j < n && chars[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    res.push_str("\\["); // unclosed '[': literal
                } else {
                    let body: String = chars[i..j].iter().collect();
                    i = j + 1;
                    res.push_str(&translate_class(&body));
                }
            }
            _ => res.push_str(&regex::escape(&c.to_string())),
        }
    }
    res
}

/// Translate a bracket-class body (the text between `[` and the closing
/// `]`, leading `!` still attached) into a `regex` character class.
fn translate_class(body: &str) -> String {
    let mut class = String::from("[");
    let mut members = body.chars();
    match members.next() {
        // A leading '!' negates the class (fnmatch spelling of regex '^').
        Some('!') => class.push('^'),
        // Everything else — including a leading '^', which fnmatch does
        // NOT treat as a negator — is an ordinary member.
        Some(first) => class.push_str(&escape_class_member(first)),
        // The scan in `translate` guarantees a non-empty body; an empty
        // class is invalid regex in both Python and the `regex` crate.
        None => {}
    }
    for member in members {
        class.push_str(&escape_class_member(member));
    }
    class.push(']');
    class
}

/// Escape a bracket-class member the `regex` crate would read differently
/// from Python's `re`: `[` opens a nested/POSIX class, `&&` is class
/// intersection, `]` closes the class early (the regex crate, unlike
/// Python, rejects a raw leading `]`), `^` would negate at the start, and
/// `\` starts an escape sequence.
fn escape_class_member(c: char) -> String {
    match c {
        '\\' | '[' | ']' | '^' | '&' => format!("\\{c}"),
        _ => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vectors from the CPython `fnmatch` module docs, evaluated with the
    /// case-sensitive (POSIX) semantics this port guarantees everywhere —
    /// `os.path.normcase` is the identity there, and `fnmatchcase` behaves
    /// the same on every platform.
    #[test]
    fn cpython_doc_vectors() {
        let cases: &[(&str, &str, bool)] = &[
            // fnmatch() docs
            ("foo.txt", "*.txt", true),
            ("foo.txt", "*.TX???", false),
            ("FOO.TXT", "*.TXT", true),
            ("FOO.TXT", "?OO.TXT", true),
            ("FOO.TXT", "FOO.*", true),
            ("FOO.TXT", "FOO.???", true),
            // fnmatchcase() docs
            ("foo.txt", "*.TXT", false),
        ];
        for (name, pattern, expected) in cases {
            assert_eq!(
                fnmatch(pattern, name),
                *expected,
                "fnmatch({pattern:?}, {name:?}) should be {expected}"
            );
        }
    }

    /// Degenerate patterns from CPython's fnmatch test suite.
    #[test]
    fn empty_and_literal_patterns() {
        assert!(fnmatch("", ""));
        assert!(!fnmatch("", "a"));
        assert!(fnmatch("*", ""));
        assert!(!fnmatch("?", ""));
        assert!(fnmatch("abc", "abc"));
        assert!(!fnmatch("abc", "abcd"));
        assert!(!fnmatch("abcd", "abc"));
    }

    /// Patterns anchor at both ends — `*.safetensors` must not match
    /// `evil.safetensors.txt`, and a bare `json` must not match
    /// `tokenizer.json`.
    #[test]
    fn patterns_are_anchored_at_both_ends() {
        assert!(!fnmatch("*.safetensors", "model.safetensors.txt"));
        assert!(!fnmatch("json", "tokenizer.json"));
        assert!(fnmatch("json", "json"));
    }

    /// `*` crosses `/` — the property that makes `*.safetensors` select
    /// sharded weights in subfolders (§7), and the reason `original/*`
    /// alone would already cover nested originals.
    #[test]
    fn star_crosses_directory_separator() {
        assert!(fnmatch("*.safetensors", "a.safetensors"));
        assert!(fnmatch("*.safetensors", "sub/dir/a.safetensors"));
        assert!(!fnmatch("*.safetensors", "sub/dir/a.safetensors.index"));
        assert!(fnmatch("original/*", "original/consolidated.safetensors"));
        assert!(fnmatch("original/*", "original/a/b/c.bin"));
        assert!(!fnmatch("original/*", "originals/a.bin"));
        // `**` is not special — identical behavior to `*` (§7 tests both)
        assert!(fnmatch("original/**", "original/a/b.bin"));
        assert_eq!(
            fnmatch("original/**", "original/a/b.bin"),
            fnmatch("original/*", "original/a/b.bin")
        );
        // Python parity: `?` matches any single character, incl. `/`.
        assert!(fnmatch("a?b", "a/b"));
    }

    /// Bracket classes, negation, and their CPython edge cases: the class
    /// ends at the first `]`, `[]]` contains `]`, `[!]x]` negates a set
    /// containing `]`, a leading `^` is a member (not a negator), and an
    /// unclosed `[` is literal.
    #[test]
    fn bracket_classes_and_negation() {
        let cases: &[(&str, &str, bool)] = &[
            ("a.txt", "[abc].txt", true),
            ("d.txt", "[abc].txt", false),
            ("d.txt", "[!abc].txt", true),
            ("a.txt", "[!abc].txt", false),
            ("5.txt", "[0-9].txt", true),
            ("x.txt", "[0-9].txt", false),
            ("x.txt", "[!0-9].txt", true),
            // meta-characters made literal by brackets
            ("*", "[*]", true),
            ("a", "[*]", false),
            // a leading ']' is a class member (CPython scan rule)
            ("]", "[]]", true),
            ("a", "[]]", false),
            // negated class containing ']' and 'x'
            ("y", "[!]x]", true),
            ("]", "[!]x]", false),
            ("x", "[!]x]", false),
            // leading '^' is a literal member, not a negator
            ("^a]", "[^]a]", true),
            ("b]", "[^]a]", false),
            // unclosed '[' is a literal '['
            ("[abc", "[abc", true),
            ("x", "[abc", false),
        ];
        for (name, pattern, expected) in cases {
            assert_eq!(
                fnmatch(pattern, name),
                *expected,
                "fnmatch({pattern:?}, {name:?}) should be {expected}"
            );
        }
    }

    /// Python `re` has no POSIX character classes, so fnmatch closes the
    /// class at the FIRST `]` and reads `[[:digit:]]` as the literal
    /// member set `[:digit` followed by a literal `]`. The `regex` crate
    /// *does* support POSIX classes — the translator escapes `[` inside
    /// class bodies so both engines agree (§7 parity).
    #[test]
    fn posix_class_syntax_stays_literal_like_python() {
        assert!(!fnmatch("[[:digit:]]", "1")); // would match if read as a POSIX class
        assert!(!fnmatch("[[:digit:]]", ":")); // class closes at the first `]`
        assert!(fnmatch("[[:digit:]]", ":]")); // member + the literal trailing `]`
        assert!(fnmatch("[[:digit:]]", "d]"));
        assert!(!fnmatch("[[:digit:]]", "1]"));
    }

    /// A glob that fails to compile into a regex (reversed range `[z-a]`)
    /// never matches instead of panicking — patterns come from
    /// `--include`/`--exclude` argv.
    #[test]
    fn uncompilable_pattern_never_matches() {
        assert!(!fnmatch("[z-a]", "a"));
        assert!(!fnmatch("[z-a]", "z"));
    }

    /// The `fnmatch.filter` doc example, through `filter_paths`.
    #[test]
    fn filter_paths_include_only() {
        let paths = ["a.py", "b.txt", "c.py"];
        let include: Vec<String> = vec!["*.py".into()];
        assert_eq!(filter_paths(&paths, &include, &[]), vec!["a.py", "c.py"]);
    }

    /// Empty include list means "everything" (§2.2); exclude still applies.
    #[test]
    fn filter_paths_empty_include_means_everything() {
        let paths = ["config.json", "model.bin", "README.md"];
        assert_eq!(
            filter_paths(&paths, &[], &[]),
            vec!["config.json", "model.bin", "README.md"]
        );
        let exclude: Vec<String> = vec!["*.bin".into(), "*.md".into()];
        assert_eq!(filter_paths(&paths, &[], &exclude), vec!["config.json"]);
    }

    /// Include is applied first, exclude last (§2.2) — an include match is
    /// still dropped when an exclude pattern hits.
    #[test]
    fn filter_paths_exclude_overrides_include() {
        let paths = ["original/consolidated.safetensors", "model.safetensors"];
        let include: Vec<String> = vec!["*.safetensors".into()];
        let exclude: Vec<String> = vec!["original/**".into()];
        assert_eq!(
            filter_paths(&paths, &include, &exclude),
            vec!["model.safetensors"]
        );
    }

    /// The preset tables are normative (§2.3) — pin them so a vLLM-drift
    /// fix stays a deliberate, one-line change.
    #[test]
    fn vllm_preset_tables_match_plan() {
        assert_eq!(
            VLLM_ALLOW.to_vec(),
            vec!["*.safetensors", "*.json", "*.txt", "*.model", "*.jinja"]
        );
        assert_eq!(
            VLLM_IGNORE.to_vec(),
            vec![
                "original/**",
                "*.bin",
                "*.pt",
                "*.gguf",
                "*.onnx",
                "onnx/**",
                "*.msgpack",
                "*.h5",
                "*.ot",
                "*.tflite",
                "*.md",
                ".gitattributes",
            ]
        );
    }

    /// Preset sanity (§2.3/§7): exactly the vLLM-relevant files survive
    /// `VLLM_ALLOW` + `VLLM_IGNORE`.
    #[test]
    fn vllm_preset_selects_expected_files() {
        let include: Vec<String> = VLLM_ALLOW.iter().map(|p| (*p).to_string()).collect();
        let exclude: Vec<String> = VLLM_IGNORE.iter().map(|p| (*p).to_string()).collect();
        let selected = |path: &str| !filter_paths(&[path], &include, &exclude).is_empty();
        // in
        assert!(selected("model.safetensors"));
        assert!(selected("model.safetensors.index.json"));
        assert!(selected("text_encoder/model-00001-of-00002.safetensors"));
        assert!(selected("config.json"));
        assert!(selected("tokenizer.model"));
        assert!(selected("chat_template.jinja"));
        // out: fallback weight formats, exports, docs, hub metadata
        assert!(!selected("model.bin"));
        assert!(!selected("pytorch_model-00001-of-00002.bin"));
        assert!(!selected("README.md"));
        assert!(!selected(".gitattributes"));
        assert!(!selected("onnx/model.onnx"));
        assert!(!selected("model.gguf"));
        // allow-matched but ignored: upstream originals never survive
        assert!(!selected("original/consolidated.safetensors"));
    }
}
