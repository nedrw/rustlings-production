//! `links` — the links in the project's documentation resolve (plan §6).
//!
//! The gate is **offline by construction**, and that is a decision rather than an oversight: whether
//! a third-party URL still answers is a fact only its server has, so a gate that asked would make a
//! step's completeness depend on somebody else's uptime — the opposite of what the course promises
//! (plan §1, §8.2: one command, offline, deterministic). The network half of link checking lives in
//! `kiln links --external`, a command no gate can call.
//!
//! What is left is still worth judging, because all of it is a property of the tree:
//!
//! * a relative target resolves, component by component, to something the project keeps;
//! * `#anchor` and `page.md#anchor` name a heading the target page has;
//! * no target is a placeholder (`TODO`, `TBD`, …) or an absolute path, and none escapes the project
//!   or walks into `target/`;
//! * the project has at least one page with at least one link — a design step whose page was never
//!   written must not pass a gate about its links (authoring.md §4.4).
//!
//! The limits are stated rather than hidden: fenced code blocks and inline code spans hold examples,
//! not links, so they are skipped; a `[label]: target` definition is checked, but `[text][label]`
//! *usages* are not resolved; `%20`-style escapes are decoded; anchors come from `#`-style headings,
//! not from setext ones.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::{KilnError, Result};
use crate::gates::GateOutcome;
use crate::gates::GateSpec;
use crate::gates::runner::GateContext;

/// How many findings a report quotes before it summarises the rest.
const FINDING_LINES: usize = 12;

/// Words that stand where a target should be.
const PLACEHOLDERS: [&str; 5] = ["todo", "tbd", "fixme", "xxx", "placeholder"];

/// Directory holding build output rather than project content.
const GENERATED_DIR: &str = "target";

/// Extensions read as markdown.
const MARKDOWN_EXTENSIONS: [&str; 2] = ["md", "markdown"];

/// Judge the markdown of the step's project.
pub fn run(ctx: &GateContext<'_>, _spec: &GateSpec) -> Result<GateOutcome> {
    if ctx.existing_project().is_none() {
        return Ok(GateOutcome::blocked(ctx.missing_project()));
    }
    judge(&ctx.project_dir)
}

/// The decision, kept apart from the gate context so a test can call it on a directory of its own.
pub fn judge(root: &Path) -> Result<GateOutcome> {
    Ok(verdict(&scan(root)?))
}

/// Everything one scan of a tree found.
#[derive(Debug)]
pub struct Scan {
    /// Markdown pages, in path order.
    pages: Vec<Page>,
    /// Targets that do not resolve, in the order they were written.
    findings: Vec<Finding>,
    /// External targets, deduplicated and sorted.
    external: Vec<String>,
    /// How many targets the pages hold.
    links: usize,
}

impl Scan {
    /// How many markdown pages were read.
    #[must_use]
    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    /// How many targets the pages hold.
    #[must_use]
    pub fn links(&self) -> usize {
        self.links
    }

    /// External targets in the order a report can walk them.
    #[must_use]
    pub fn external(&self) -> &[String] {
        &self.external
    }

    /// What did not resolve.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// `true` when every target resolved.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// One markdown file the scan read.
#[derive(Debug)]
struct Page {
    /// Absolute path on disk.
    path: PathBuf,
    /// Path relative to the scanned root, `/`-separated, as a report writes it.
    relative: String,
    /// Targets in the order they were written.
    links: Vec<Link>,
    /// Slugs of its headings, for anchor targets.
    anchors: Vec<String>,
}

/// One target written in a page.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    /// 1-based line the target is written on.
    line: usize,
    /// Target as written, without its optional title.
    target: String,
}

/// One target that does not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Page the target is written on, relative to the scanned root.
    pub page: String,
    /// 1-based line.
    pub line: usize,
    /// Target as written.
    pub target: String,
    /// What is wrong with it, phrased as the thing to do about it.
    pub problem: String,
}

impl Finding {
    fn new(page: &Page, link: &Link, problem: String) -> Self {
        Self {
            page: page.relative.clone(),
            line: link.line,
            target: link.target.clone(),
            problem,
        }
    }

    /// One line a report can print: ``docs/runbook.md:12 `TODO` — … ``.
    #[must_use]
    pub fn line(&self) -> String {
        format!(
            "{}:{} `{}` — {}",
            self.page, self.line, self.target, self.problem
        )
    }
}

/// Turn what a scan found into the gate's verdict.
fn verdict(scan: &Scan) -> GateOutcome {
    if scan.pages() == 0 {
        return GateOutcome::failed(
            "no markdown in this project: this step's deliverable is a page a human reads, so a gate \
             about its links has nothing to judge (authoring.md §4.4)",
        );
    }
    if scan.is_clean() && scan.links() == 0 {
        return GateOutcome::failed(format!(
            "{} page(s) and not one link between them: a runbook points at the drill, the signals \
             and the endpoint it talks about — write the targets you would follow at 3 a.m.",
            scan.pages()
        ));
    }
    if scan.is_clean() {
        return GateOutcome::passed(format!(
            "{} link(s) in {} page(s) resolve; {} external target(s) are listed but not fetched, \
             because this gate never goes to the network (`kiln links --external` does)",
            scan.links(),
            scan.pages(),
            scan.external().len()
        ));
    }
    GateOutcome::failed(format!(
        "{} of {} link(s) do not resolve:\n    {}",
        scan.findings().len(),
        scan.links(),
        list(scan.findings())
    ))
}

/// Findings as report lines, cut off with a count rather than an ellipsis.
fn list(findings: &[Finding]) -> String {
    let shown = findings
        .iter()
        .take(FINDING_LINES)
        .map(Finding::line)
        .collect::<Vec<String>>()
        .join("\n    ");
    match findings.len().checked_sub(FINDING_LINES) {
        Some(rest) if rest > 0 => format!("{shown}\n    …and {rest} more"),
        _ => shown,
    }
}

/// Read every markdown page under `root` and judge its targets.
pub fn scan(root: &Path) -> Result<Scan> {
    let mut pages: Vec<Page> = Vec::new();
    for path in markdown_files(root)? {
        let text = read(&path)?;
        pages.push(Page {
            relative: relative_name(root, &path),
            links: extract(&text),
            anchors: anchors(&text),
            path,
        });
    }

    let mut findings = Vec::new();
    let mut external = BTreeSet::new();
    let mut links = 0;
    for page in &pages {
        for link in &page.links {
            links += 1;
            match classify(&link.target) {
                Err(problem) => findings.push(Finding::new(page, link, problem)),
                Ok(Target::External(url)) => {
                    external.insert(url);
                }
                Ok(Target::Anchor(anchor)) => {
                    if !names_heading(&page.anchors, &anchor) {
                        findings.push(Finding::new(
                            page,
                            link,
                            format!("this page has no heading `#{anchor}`"),
                        ));
                    }
                }
                Ok(Target::Relative { path, anchor }) => {
                    judge_relative(
                        root,
                        &pages,
                        page,
                        link,
                        &path,
                        anchor.as_deref(),
                        &mut findings,
                    );
                }
            }
        }
    }

    Ok(Scan {
        pages,
        findings,
        external: external.into_iter().collect(),
        links,
    })
}

/// Judge one relative target: where it lands, and — for markdown — the heading it names.
fn judge_relative(
    root: &Path,
    pages: &[Page],
    page: &Page,
    link: &Link,
    target: &str,
    anchor: Option<&str>,
    findings: &mut Vec<Finding>,
) {
    let from = page.path.parent().unwrap_or(root);
    match resolve(root, from, target) {
        Resolved::Missing => findings.push(Finding::new(
            page,
            link,
            format!("`{target}` does not exist in this project"),
        )),
        Resolved::WrongCase(actual) => findings.push(Finding::new(
            page,
            link,
            format!(
                "`{target}` differs in case from `{actual}` — that spelling only works on a \
                 case-insensitive disk"
            ),
        )),
        Resolved::Escapes => findings.push(Finding::new(
            page,
            link,
            format!("`{target}` points outside the project: a page has to stand on its own"),
        )),
        Resolved::Generated => findings.push(Finding::new(
            page,
            link,
            format!("`{target}` points into `{GENERATED_DIR}/`, which is build output"),
        )),
        Resolved::Found(path) => {
            let Some(anchor) = anchor else {
                return;
            };
            // Only a page this scan read can have its headings checked; a target outside the
            // markdown (an image, a directory) is judged by existing.
            let Some(target) = pages.iter().find(|candidate| candidate.path == path) else {
                return;
            };
            if !names_heading(&target.anchors, anchor) {
                let named = &target.relative;
                findings.push(Finding::new(
                    page,
                    link,
                    format!("`{named}` has no heading `#{anchor}`"),
                ));
            }
        }
    }
}
/// `true` when one of the slugs is the anchor a link asked for.
fn names_heading(anchors: &[String], anchor: &str) -> bool {
    anchors.iter().any(|slug| slug == anchor)
}

/// Every markdown file under `root`, sorted so two scans of one tree agree.
fn markdown_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    Ok(files)
}

/// Collect markdown files, skipping what is not the project.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let unreadable = |source| KilnError::Read {
        path: dir.to_path_buf(),
        source,
    };
    let entries = std::fs::read_dir(dir).map_err(unreadable)?;
    for entry in entries {
        let entry = entry.map_err(unreadable)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // A symlink is not followed: the answer would depend on what is on the other side of it,
        // which is exactly the kind of dependency this gate exists to avoid.
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if name.starts_with('.') || name == GENERATED_DIR {
                continue;
            }
            walk(&entry.path(), files)?;
            continue;
        }
        if is_markdown(&name) {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// `true` for a file the course reads as markdown.
fn is_markdown(name: &str) -> bool {
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    MARKDOWN_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

/// Read a page as text, replacing bytes that are not UTF-8 rather than failing on them.
fn read(path: &Path) -> Result<String> {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|source| KilnError::Read {
            path: path.to_path_buf(),
            source,
        })
}

/// How a path is named in a report: relative to the scanned root, `/`-separated.
fn relative_name(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// What a target names.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// A URL this check lists and never fetches.
    External(String),
    /// `#heading` in the page itself.
    Anchor(String),
    /// A path inside the project, with the anchor it names if any.
    Relative {
        /// Decoded path, as written.
        path: String,
        /// Decoded anchor, lower case.
        anchor: Option<String>,
    },
}

/// Decide what a target is, or say what is wrong with it.
fn classify(target: &str) -> std::result::Result<Target, String> {
    let target = target.trim();
    if target.is_empty() {
        return Err("the target is empty".to_owned());
    }
    let bare = target.trim_start_matches('#');
    if PLACEHOLDERS
        .iter()
        .any(|word| bare.eq_ignore_ascii_case(word))
    {
        return Err(format!(
            "`{target}` is a placeholder: write the target you would actually follow"
        ));
    }
    let lowered = target.to_ascii_lowercase();
    if ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lowered.starts_with(scheme))
    {
        return Ok(Target::External(target.to_owned()));
    }
    if let Some((scheme, _)) = target.split_once(':')
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return Err(format!(
            "`{target}` uses the `{scheme}:` scheme; this check follows http, https and mailto"
        ));
    }
    if let Some(anchor) = target.strip_prefix('#') {
        let anchor = percent_decode(anchor).to_lowercase();
        if anchor.trim().is_empty() {
            return Err(format!("`{target}` names no heading at all"));
        }
        return Ok(Target::Anchor(anchor));
    }
    if target.starts_with('/') {
        return Err(format!(
            "`{target}` is an absolute path: write it relative to the page"
        ));
    }
    let (path, anchor) = match target.split_once('#') {
        Some((path, anchor)) => (path, Some(percent_decode(anchor).to_lowercase())),
        None => (target, None),
    };
    Ok(Target::Relative {
        path: percent_decode(path),
        anchor,
    })
}

/// Where one relative target lands.
#[derive(Debug, PartialEq, Eq)]
enum Resolved {
    /// A path the project keeps.
    Found(PathBuf),
    /// Nothing of that name.
    Missing,
    /// The name exists under a different spelling, which only works on this disk.
    WrongCase(String),
    /// The target walks out of the scanned root.
    Escapes,
    /// The target points into build output.
    Generated,
}

/// Resolve a target one component at a time against the directory listing.
///
/// Exactly, rather than through `Path::exists`: a link that only resolves because this machine's disk
/// ignores case would go red on the reviewer's machine, and a check whose answer depends on the file
/// system is not one the course may promise (plan §1).
fn resolve(root: &Path, from: &Path, target: &str) -> Resolved {
    let mut current = from.to_path_buf();
    for component in target.split('/') {
        match component {
            "" | "." => continue,
            ".." => {
                if !current.pop() || !current.starts_with(root) {
                    return Resolved::Escapes;
                }
            }
            name => {
                if name == GENERATED_DIR {
                    return Resolved::Generated;
                }
                match lookup(&current, name) {
                    Some(found) => current = found,
                    None => {
                        return match near_miss(&current, name) {
                            Some(actual) => {
                                let mut corrected = current.clone();
                                corrected.push(&actual);
                                Resolved::WrongCase(relative_name(root, &corrected))
                            }
                            None => Resolved::Missing,
                        };
                    }
                }
            }
        }
    }
    Resolved::Found(current)
}

/// `dir/name`, if a directory entry carries that name exactly.
fn lookup(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|entry| entry.file_name().to_str() == Some(name))
        .map(|entry| entry.path())
}

/// The entry that differs from `name` only in case, if there is one.
fn near_miss(dir: &Path, name: &str) -> Option<String> {
    let lowered = name.to_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let found = entry.file_name();
        let found = found.to_str()?;
        (found.to_lowercase() == lowered).then(|| found.to_owned())
    })
}

/// Decode `%20`-style escapes, so a target copied out of a browser still names the file on disk.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(byte) = bytes.get(index) {
        if *byte == b'%'
            && let (Some(high), Some(low)) = (bytes.get(index + 1), bytes.get(index + 2))
            && let (Some(high), Some(low)) = (hex(*high), hex(*low))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        decoded.push(*byte);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Value of one hexadecimal digit.
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Targets written in one markdown text, in the order they appear.
fn extract(text: &str) -> Vec<Link> {
    let mut links = Vec::new();
    for (number, line) in prose_lines(text) {
        let line = blank_code_spans(line);
        if let Some(target) = definition_target(&line) {
            links.push(Link {
                line: number,
                target: target.to_owned(),
            });
            continue;
        }
        inline_links(&line, number, &mut links);
        for url in autolinks(&line) {
            links.push(Link {
                line: number,
                target: url.to_owned(),
            });
        }
    }
    links
}

/// The lines worth looking at, with fenced code blocks removed.
fn prose_lines(text: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut fence: Option<&str> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"]
            .iter()
            .find(|marker| trimmed.starts_with(**marker))
            .copied();
        match (fence, marker) {
            // A block closes on its own marker; another marker inside it is content.
            (Some(open), Some(closed)) if open == closed => fence = None,
            (Some(_), _) => continue,
            (None, Some(open)) => fence = Some(open),
            (None, None) => lines.push((index + 1, line)),
        }
    }
    lines
}

/// Replace inline code spans with spaces: ``write `](x.md)` here`` shows an example, not a link.
fn blank_code_spans(line: &str) -> String {
    let mut blanked = String::with_capacity(line.len());
    let mut inside = false;
    for character in line.chars() {
        if character == '`' {
            inside = !inside;
            blanked.push(' ');
            continue;
        }
        blanked.push(if inside { ' ' } else { character });
    }
    blanked
}

/// Targets written as `[text](target)` or `![alt](target)`.
fn inline_links(line: &str, number: usize, links: &mut Vec<Link>) {
    let mut from = 0;
    while let Some(found) = line.get(from..).and_then(|rest| rest.find("](")) {
        let open = from + found + 1;
        let Some(after) = line.get(open + 1..) else {
            return;
        };
        let mut depth = 1usize;
        let mut close = None;
        for (index, character) in after.char_indices() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + 1 + index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            return;
        };
        if let Some(target) = line.get(open + 1..close) {
            links.push(Link {
                line: number,
                target: bare_target(target).to_owned(),
            });
        }
        from = close + 1;
    }
}

/// Targets written as `<https://…>`, the form that needs no link text.
fn autolinks(line: &str) -> Vec<&str> {
    let mut urls = Vec::new();
    let mut from = 0;
    while let Some(found) = line.get(from..).and_then(|rest| rest.find('<')) {
        let open = from + found;
        let Some(after) = line.get(open + 1..) else {
            break;
        };
        let Some(end) = after.find('>') else {
            break;
        };
        if let Some(inner) = after.get(..end) {
            let lowered = inner.to_ascii_lowercase();
            if ["http://", "https://", "mailto:"]
                .iter()
                .any(|scheme| lowered.starts_with(scheme))
            {
                urls.push(inner);
            }
        }
        from = open + 1 + end + 1;
    }
    urls
}

/// The target of a `[label]: target` definition line, if this is one.
fn definition_target(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('[')?;
    let (label, rest) = rest.split_once("]:")?;
    if label.is_empty() || label.starts_with('^') {
        // `[^1]:` is a footnote definition, and a footnote is not a link.
        return None;
    }
    Some(bare_target(rest))
}

/// A target without its optional title and without the angle brackets that let it hold spaces.
fn bare_target(raw: &str) -> &str {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('<') {
        return rest
            .split_once('>')
            .map_or(rest, |(target, _)| target.trim());
    }
    raw.split_whitespace().next().unwrap_or_default()
}

/// Slugs of the headings of one markdown text, the way a `#anchor` link has to spell them.
fn anchors(text: &str) -> Vec<String> {
    let mut slugs: Vec<String> = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (_number, line) in prose_lines(text) {
        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if hashes == 0 || hashes > 6 {
            continue;
        }
        let title = trimmed.chars().skip(hashes).collect::<String>();
        if !title.is_empty() && !title.starts_with(' ') {
            continue;
        }
        let slug = slug(&title);
        if slug.is_empty() {
            continue;
        }
        // GitHub numbers repeated headings, and a link is written against the numbered slug.
        let count = seen.entry(slug.clone()).or_insert(0);
        let unique = if *count == 0 {
            slug
        } else {
            format!("{slug}-{count}")
        };
        *count += 1;
        slugs.push(unique);
    }
    slugs
}

/// One heading as its slug: lower case, spaces as `-`, punctuation dropped.
fn slug(heading: &str) -> String {
    let heading = heading.trim().trim_end_matches('#').trim();
    let mut slug = String::with_capacity(heading.len());
    let mut characters = heading.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            // `[text](target)`: the text is part of the heading, the target is not.
            ']' => {
                if characters.peek() == Some(&'(') {
                    let mut depth = 0usize;
                    for skipped in characters.by_ref() {
                        match skipped {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            // `<tag>`: markup, not text.
            '<' => {
                for skipped in characters.by_ref() {
                    if skipped == '>' {
                        break;
                    }
                }
            }
            _ => push_significant(&mut slug, character),
        }
    }
    slug.trim_matches('-').to_owned()
}

/// Keep what a slug keeps: letters, digits, `-` and `_`; whitespace becomes `-`.
fn push_significant(slug: &mut String, character: char) {
    if character.is_whitespace() {
        slug.push('-');
        return;
    }
    for lowered in character.to_lowercase() {
        if lowered.is_alphanumeric() || lowered == '-' || lowered == '_' {
            slug.push(lowered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gates::Outcome;
    use crate::test_support::TempDir;

    /// Targets of one text, in the order the scanner read them.
    fn targets(text: &str) -> Vec<String> {
        extract(text).into_iter().map(|link| link.target).collect()
    }

    /// Problems one tree reports, as report lines.
    fn problems(root: &Path) -> Vec<String> {
        scan(root)
            .expect("the tree should be readable")
            .findings()
            .iter()
            .map(Finding::line)
            .collect()
    }

    #[test]
    fn inline_links_are_read_with_their_titles_and_brackets() {
        let text =
            "[a](b.md) and ![alt](images/p.png) and [t](c.md \"the title\") and [s](<d e.md>)";
        assert_eq!(targets(text), ["b.md", "images/p.png", "c.md", "d e.md"]);
    }

    #[test]
    fn examples_inside_code_are_not_links() {
        let text = "```\n[fenced](missing.md)\n```\n\nWrite `](example.md)` to show a link.\n\n[real](here.md)\n";
        assert_eq!(targets(text), ["here.md"]);
    }

    #[test]
    fn autolinks_and_definitions_are_read_too() {
        let text = "See <https://example.com/a> and the definition below.\n\n[slo]: https://example.com/b \"the title\"\n";
        assert_eq!(
            targets(text),
            ["https://example.com/a", "https://example.com/b"]
        );
    }

    #[test]
    fn anchors_come_from_headings_the_way_a_link_has_to_spell_them() {
        let text = "# Title\n\n## 6. 闸门\n\n### Trailing ##\n\n## `code` and *emphasis*\n\n## Repeat\n\n## Repeat\n\n#Tag\n\n```\n## Not a heading\n```\n";
        assert_eq!(
            anchors(text),
            [
                "title",
                "6-闸门",
                "trailing",
                "code-and-emphasis",
                "repeat",
                "repeat-1"
            ]
        );
    }

    #[test]
    fn a_target_is_classified_or_rejected_with_a_reason() {
        assert_eq!(
            classify("https://example.com/x"),
            Ok(Target::External("https://example.com/x".to_owned()))
        );
        assert_eq!(classify("#Steps"), Ok(Target::Anchor("steps".to_owned())));
        assert_eq!(
            classify("docs/runbook.md#steps"),
            Ok(Target::Relative {
                path: "docs/runbook.md".to_owned(),
                anchor: Some("steps".to_owned())
            })
        );
        assert_eq!(
            classify("runbook%20copy.md"),
            Ok(Target::Relative {
                path: "runbook copy.md".to_owned(),
                anchor: None
            })
        );
        for rejected in [
            "",
            "TODO",
            "#TODO",
            "tbd",
            "/etc/passwd",
            "ftp://example.com",
        ] {
            assert!(classify(rejected).is_err(), "{rejected} should be rejected");
        }
    }

    #[test]
    fn a_target_that_leaves_the_project_or_names_build_output_is_reported() {
        let temp = TempDir::new("links-resolve");
        temp.write("docs/runbook.md", "# Runbook\n");
        temp.write("tests/drill.rs", "");
        let root = temp.path();
        let from = root.join("docs");

        assert_eq!(
            resolve(root, &from, "runbook.md"),
            Resolved::Found(root.join("docs/runbook.md"))
        );
        assert_eq!(
            resolve(root, &from, "../tests/drill.rs"),
            Resolved::Found(root.join("tests/drill.rs"))
        );
        assert_eq!(resolve(root, &from, "../../etc/passwd"), Resolved::Escapes);
        assert_eq!(
            resolve(root, &from, "target/doc/x.html"),
            Resolved::Generated
        );
        assert_eq!(resolve(root, &from, "missing.md"), Resolved::Missing);
        assert_eq!(
            resolve(root, &from, "Runbook.md"),
            Resolved::WrongCase("docs/runbook.md".to_owned())
        );
    }

    #[test]
    fn a_page_whose_links_resolve_passes() {
        let temp = TempDir::new("links-pass");
        temp.write("tests/drill.rs", "");
        temp.write(
            "docs/runbook.md",
            "# Runbook\n\n## Steps\n\nSee [the drill](../tests/drill.rs) and <https://example.com/docs>.\n\nBack to [the steps](#steps).\n",
        );

        let scan = scan(temp.path()).expect("the tree should be readable");
        assert!(scan.is_clean(), "{:?}", scan.findings());
        assert_eq!(scan.pages(), 1);
        assert_eq!(scan.links(), 3);
        assert_eq!(scan.external(), ["https://example.com/docs"]);
        assert_eq!(verdict(&scan).outcome, Outcome::Passed);
    }

    #[test]
    fn a_target_that_does_not_resolve_names_its_page_and_line() {
        let temp = TempDir::new("links-missing");
        temp.write(
            "docs/runbook.md",
            "# Runbook\n\nSee [the drill](drill.rs).\n",
        );

        let problems = problems(temp.path());
        assert_eq!(problems.len(), 1);
        assert_eq!(
            problems.first().map(String::as_str),
            Some("docs/runbook.md:3 `drill.rs` — `drill.rs` does not exist in this project")
        );
    }

    #[test]
    fn an_anchor_that_names_no_heading_is_reported() {
        let temp = TempDir::new("links-anchor");
        temp.write(
            "docs/runbook.md",
            "# Runbook\n\n## Steps\n\n[gone](#rollback)\n",
        );
        temp.write("docs/slo.md", "# SLO\n\n## Budget\n");
        temp.write(
            "docs/plan.md",
            "# Plan\n\n[slo](slo.md#budget) and [typo](slo.md#target)\n",
        );

        let problems = problems(temp.path());
        assert_eq!(problems.len(), 2);
        // Pages are judged in path order, so `plan.md` comes before `runbook.md`.
        assert!(
            problems[0].contains("no heading `#target`"),
            "{}",
            problems[0]
        );
        assert!(problems[1].contains("#rollback"), "{}", problems[1]);
    }

    #[test]
    fn a_target_that_escapes_the_project_is_reported() {
        let temp = TempDir::new("links-escape");
        temp.write(
            "docs/runbook.md",
            "# Runbook\n\n[course notes](../../docs/plan.md)\n\n[build](target/doc/x.html)\n",
        );

        let problems = problems(temp.path());
        assert_eq!(problems.len(), 2);
        assert!(
            problems[0].contains("outside the project"),
            "{}",
            problems[0]
        );
        assert!(problems[1].contains("build output"), "{}", problems[1]);
    }

    #[test]
    fn a_project_without_pages_or_without_links_cannot_pass() {
        let empty = TempDir::new("links-empty");
        empty.write("src/main.rs", "fn main() {}\n");
        let outcome = judge(empty.path()).expect("the tree should be readable");
        assert_eq!(outcome.outcome, Outcome::Failed);
        assert!(outcome.detail.contains("no markdown"), "{}", outcome.detail);

        let quiet = TempDir::new("links-quiet");
        quiet.write(
            "docs/runbook.md",
            "# Runbook\n\nProse, and nothing to click.\n",
        );
        let outcome = judge(quiet.path()).expect("the tree should be readable");
        assert_eq!(outcome.outcome, Outcome::Failed);
        assert!(
            outcome.detail.contains("not one link"),
            "{}",
            outcome.detail
        );
    }

    #[test]
    fn build_output_and_hidden_directories_hold_no_pages() {
        let temp = TempDir::new("links-skipped");
        temp.write("target/notes.md", "[gone](missing.md)\n");
        temp.write(".git/notes.md", "[gone](missing.md)\n");
        temp.write(
            "docs/runbook.md",
            "# Runbook\n\n[drill](../tests/drill.rs)\n",
        );
        temp.write("tests/drill.rs", "");

        let scan = scan(temp.path()).expect("the tree should be readable");
        assert_eq!(scan.pages(), 1);
        assert!(scan.is_clean(), "{:?}", scan.findings());
    }
}
