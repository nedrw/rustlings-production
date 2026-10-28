//! The documentation site: the course, in every locale, as static HTML (plan §11, M5).
//!
//! The site is a *view* of the course, not a second copy of it: every page is rendered from the same
//! step bodies `kiln hint` reads, resolved through the same fallback chain, so a translation that is
//! missing shows the English page and a body that was never written is an error here too.
//!
//! Three decisions worth naming:
//!
//! * **One implementation, like everything else.** `kiln site` is what a maintainer runs and what
//!   `cargo xtask ci` runs (`--check`): CI renders every page into a scratch directory and throws it
//!   away, so a body that does not render fails the build before anybody deploys a broken link.
//! * **No server, no index library.** The output is files a static host can serve: HTML, one
//!   stylesheet, one JSON search index and twenty lines of JavaScript that filter it. A course has
//!   238 pages; anything heavier is a dependency the next maintainer has to keep alive.
//! * **The site does not translate anything.** Track titles live in `track.toml` and stay as
//!   authored; step titles come from each body's front matter, which is where a translator already
//!   wrote them. Nothing is invented to fill a gap.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{KilnError, Result};
use crate::i18n::ui::Ui;
use crate::i18n::{self, Locale, StepDoc};
use crate::info::{Course, Step, Track};

mod render;

/// Directory under the repository root that `kiln site` writes by default.
pub const DEFAULT_OUT: &str = "site";

/// How to build the site.
#[derive(Debug, Clone)]
pub struct Options {
    /// Directory the site is written into.
    pub out: PathBuf,
    /// Locales to render, in navigation order. Empty means every locale with a step directory.
    pub locales: Vec<Locale>,
    /// Render into a scratch directory, report, and remove it again (what CI runs).
    pub check: bool,
}

impl Options {
    /// Build the site into `out`, in every authored locale.
    pub fn new(out: impl Into<PathBuf>) -> Self {
        Self {
            out: out.into(),
            locales: Vec::new(),
            check: false,
        }
    }

    /// Render everything into a scratch directory and throw it away: the shape `xtask ci` runs.
    #[must_use]
    pub fn check() -> Self {
        Self {
            out: PathBuf::new(),
            locales: Vec::new(),
            check: true,
        }
    }
}

/// What a build produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Locales rendered, in navigation order.
    pub locales: Vec<Locale>,
    /// Pages written, the locale picker included.
    pub pages: usize,
    /// Directory the site was written to (removed again when `check` was set).
    pub out: PathBuf,
}

/// Render the course as a static site.
pub fn build(course: &Course, options: &Options) -> Result<Report> {
    let source = Locale::parse(&course.manifest().default_locale)?;
    let locales = if options.locales.is_empty() {
        locales_of(course, &source)?
    } else {
        options.locales.clone()
    };
    if locales.is_empty() {
        return Err(KilnError::Invalid(
            "no locale has a step directory under course/docs/steps — nothing to publish"
                .to_owned(),
        ));
    }
    let out = if options.check {
        scratch_dir()
    } else if options.out.as_os_str().is_empty() {
        course.root().join(DEFAULT_OUT)
    } else {
        options.out.clone()
    };
    std::fs::create_dir_all(&out).map_err(|source| KilnError::Write {
        path: out.clone(),
        source,
    })?;
    let pages = write_site(course, &locales, &source, &out)?;
    if options.check {
        check_links(&out)?;
    }
    if options.check {
        let _ = std::fs::remove_dir_all(&out);
    }
    Ok(Report {
        locales,
        pages,
        out,
    })
}

/// The locales the course has bodies for: every directory under `docs/steps/` that names one,
/// with the source locale first and the rest in name order.
fn locales_of(course: &Course, source: &Locale) -> Result<Vec<Locale>> {
    let steps = course.dir().join("docs/steps");
    let entries = std::fs::read_dir(&steps).map_err(|error| KilnError::Read {
        path: steps.clone(),
        source: error,
    })?;
    let mut locales: Vec<Locale> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| KilnError::Read {
            path: steps.clone(),
            source: error,
        })?;
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Ok(locale) = Locale::parse(&name) {
            locales.push(locale);
        }
    }
    locales.sort_by_key(|locale| (locale != source, locale.as_str().to_owned()));
    Ok(locales)
}

/// Write every page and return how many there are.
fn write_site(course: &Course, locales: &[Locale], source: &Locale, out: &Path) -> Result<usize> {
    let mut pages = 0;
    write_file(&out.join("assets/style.css"), STYLE)?;
    write_file(&out.join("assets/search.js"), SEARCH)?;

    let mut index: Vec<SearchEntry> = Vec::new();
    for locale in locales {
        let catalogue = Ui::load(&course.dir(), locale, source)?;
        let home = Page {
            ui: &catalogue,
            locale: Some(locale),
            below: "index.html",
            depth: 1,
            locales,
            title: "rustlings-production",
        };
        write_file(
            &out.join(locale.as_str()).join("index.html"),
            &home.home(course, source),
        )?;
        pages += 1;

        for track in course.tracks() {
            let track_below = format!("{}/index.html", track.id);
            let track_page = Page {
                ui: &catalogue,
                locale: Some(locale),
                below: &track_below,
                depth: 2,
                locales,
                title: track.title_in(locale.as_str()),
            };
            write_file(
                &out.join(locale.as_str()).join(&track.id).join("index.html"),
                &track_page.overview(track, locale, source, course),
            )?;
            pages += 1;

            for (position, step) in track.step.iter().enumerate() {
                let body = step_body(course, step, locale, source)?;
                let title = body_title(&body, step);
                index.push(SearchEntry {
                    title: title.clone(),
                    url: format!("{}/{}/{}.html", locale.as_str(), track.id, step.id),
                    track: track.id.clone(),
                    step: format!("{}/{}", track.id, step.id),
                    text: excerpt(&body.text),
                });
                let step_below = format!("{}/{}.html", track.id, step.id);
                let page = Page {
                    ui: &catalogue,
                    locale: Some(locale),
                    below: &step_below,
                    depth: 2,
                    locales,
                    title: &title,
                };
                let previous = position
                    .checked_sub(1)
                    .and_then(|before| track.step.get(before));
                let next = track.step.get(position + 1);
                write_file(
                    &out.join(locale.as_str())
                        .join(&track.id)
                        .join(format!("{}.html", step.id)),
                    &page.step(track, step, &body, locale, source, previous, next),
                )?;
                pages += 1;
            }
        }
    }

    let source_catalogue = Ui::load(&course.dir(), source, source)?;
    let picker = Page {
        ui: &source_catalogue,
        locale: None,
        below: "index.html",
        depth: 0,
        locales,
        title: "rustlings-production",
    };
    write_file(&out.join("index.html"), &picker.picker())?;
    pages += 1;

    let json = serde_json::to_string(&index)
        .map_err(|error| KilnError::Invalid(format!("cannot encode search.json: {error}")))?;
    write_file(&out.join("search.json"), &json)?;
    Ok(pages)
}

/// Refuse a body line that starts with `<`: the renderer would read it as an HTML block.
///
/// Markdown has no HTML in this course, and the one way in is a *wrapped* inline code span whose
/// continuation happens to begin with `<DIR>`. CommonMark then opens an HTML block, the paragraph
/// ends mid-sentence and the rest of the line is passed through unescaped — the page looks almost
/// right, which is the worst kind of wrong. The fix is to rewrap the source, so that is what the
/// message asks for.
fn check_line_starts(step: &Step, path: &Path, text: &str) -> Result<()> {
    let mut in_fence = false;
    for (number, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && line.starts_with('<') {
            return Err(KilnError::Invalid(format!(
                "{}:{} ({}): a line may not start with `<` — the site renderer reads it as raw \
                 HTML and the paragraph ends mid-sentence; rewrap the code span",
                path.display(),
                number + 1,
                step.id
            )));
        }
    }
    Ok(())
}

/// The body a locale resolves to, or an error naming the step that has none in any locale.
fn step_body(
    course: &Course,
    step: &Step,
    locale: &Locale,
    source: &Locale,
) -> Result<i18n::StepBody> {
    match i18n::resolve_step_doc(&course.dir(), &step.entry, locale, source)? {
        StepDoc::Found { path, .. } => {
            let body = i18n::read_step_body(&path)?;
            check_line_starts(step, &path, &body.text)?;
            Ok(body)
        }
        StepDoc::Missing { path } => Err(KilnError::NotAuthored {
            what: format!("the body of `{}` ({})", step.id, path.display()),
        }),
    }
}

/// The title a page shows: the body's own (translated) title, else the metadata's.
fn body_title(body: &i18n::StepBody, step: &Step) -> String {
    body.front_matter
        .title
        .clone()
        .unwrap_or_else(|| step.title.clone())
}

/// A short prose excerpt for the search index: prose only, whitespace squeezed, cut at a word.
fn excerpt(text: &str) -> String {
    let mut words: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence
            || trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with('|')
            || trimmed.starts_with("+++")
        {
            continue;
        }
        words.extend(
            trimmed
                .split_whitespace()
                .map(|word| word.trim_matches(|c| c == '`' || c == '*')),
        );
    }
    let joined = words.join(" ");
    let mut excerpt = String::new();
    let mut length = 0;
    for word in joined.split_whitespace() {
        if length + word.chars().count() > EXCERPT {
            break;
        }
        if !excerpt.is_empty() {
            excerpt.push(' ');
        }
        excerpt.push_str(word);
        length += word.chars().count() + 1;
    }
    excerpt
}

/// Characters of prose the search index keeps per step.
const EXCERPT: usize = 200;

/// One row of `search.json`.
#[derive(Debug, Serialize)]
struct SearchEntry {
    /// The title the step page shows.
    title: String,
    /// Site-root-relative URL of the page.
    url: String,
    /// Track id.
    track: String,
    /// Step id (`mailer/1.1-workspace`).
    step: String,
    /// The first prose of the body, for matching more than the title.
    text: String,
}

/// The site's own words — labels and sentences, not course content.
///
/// They live where the CLI's words live: English in the code, overrides in
/// `course/locales/<locale>/ui.toml` (`i18n::ui::Ui`, authoring.md §8.1). A locale that translates
/// nothing still gets every page; it simply reads its furniture in English. Two keys are HTML
/// fragments rather than text — `site.fallback` and `site.footer` — because they wrap a locale tag
/// and a link.
const DEFAULT_TRACKS: &str = "Tracks";
const DEFAULT_PICKER: &str = "Choose a language for the course.";
const DEFAULT_SEARCH: &str = "search…";
const DEFAULT_UNIT: &str = "step(s)";
const DEFAULT_PROJECT: &str = "project";
const DEFAULT_FALLBACK: &str =
    "no <code>{locale}</code> body for this step yet — this page is in <code>{found}</code>.";
const DEFAULT_SOURCE_NOTE: &str = "in English — the only locale this text is written in";
const DEFAULT_MINUTES: &str = "min";
const DEFAULT_FOOTER: &str = "rendered by <code>kiln site</code> · \
                              <a href=\"https://github.com/nedrw/rustlings-production\">source</a>";

/// A language's name as its own speakers write it, for the picker.
fn language_name(locale: &Locale) -> String {
    match locale.as_str() {
        "en" => "English".to_owned(),
        "zh-Hans" => "中文（简体）".to_owned(),
        other => other.to_owned(),
    }
}

/// One page's position in the site: what it links to and what it is called.
struct Page<'a> {
    /// The locale's UI catalogue: the site's labels and the manifest's messages.
    ui: &'a Ui,
    /// Locale the page is in, or `None` for the locale picker at the site root.
    locale: Option<&'a Locale>,
    /// Path below the locale directory (`mailer/1.1-workspace.html`) or `index.html`.
    below: &'a str,
    /// How many directories separate the page from the site root.
    depth: usize,
    /// Every locale the site published, for the language switcher.
    locales: &'a [Locale],
    /// The page title.
    title: &'a str,
}

impl Page<'_> {
    /// Path back to the site root.
    fn root(&self) -> String {
        "../".repeat(self.depth)
    }

    /// The same page in another locale, when the site published one.
    fn in_locale(&self, locale: &Locale) -> String {
        let root = self.root();
        let below = self.locale.map_or("index.html", |_| self.below);
        format!("{root}{}/{below}", locale.as_str())
    }

    /// Wrap a body in the header, the switcher and the search box.
    fn layout(&self, body: &str) -> String {
        let ui = self.ui;
        let root = self.root();
        let css = format!("{root}assets/style.css");
        let search = format!("{root}assets/search.js");
        let index = format!("{root}search.json");
        let mut html = String::with_capacity(body.len() + 2048);
        html.push_str("<!doctype html>\n<html lang=\"");
        html.push_str(self.locale.map_or("en", Locale::as_str));
        html.push_str("\">\n<head>\n<meta charset=\"utf-8\">\n");
        html.push_str(
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>",
        );
        html.push_str(&render::escape(self.title));
        html.push_str("</title>\n<link rel=\"stylesheet\" href=\"");
        html.push_str(&css);
        html.push_str("\">\n</head>\n<body>\n<header>\n");
        html.push_str("<a class=\"brand\" href=\"");
        match self.locale {
            Some(_) => html.push_str(&format!("{root}index.html")),
            None => html.push_str("index.html"),
        }
        html.push_str("\">rustlings-production</a>\n<nav class=\"langs\">");
        for locale in self.locales {
            if self.locale == Some(locale) {
                html.push_str(&format!("<span>{}</span>", locale.as_str()));
            } else {
                html.push_str(&format!(
                    "<a href=\"{}\">{}</a>",
                    self.in_locale(locale),
                    locale.as_str()
                ));
            }
        }
        html.push_str(
            "</nav>\n<div class=\"search\"><input id=\"q\" type=\"search\" placeholder=\"",
        );
        html.push_str(&render::escape(ui.text("site.search", DEFAULT_SEARCH)));
        html.push_str("\" autocomplete=\"off\" data-index=\"");
        html.push_str(&index);
        html.push_str("\"><ol id=\"results\" hidden></ol></div>\n</header>\n<main>\n");
        html.push_str(body);
        html.push_str("</main>\n<footer><p>");
        html.push_str(ui.text("site.footer", DEFAULT_FOOTER));
        html.push_str("</p></footer>\n<script src=\"");
        html.push_str(&search);
        html.push_str("\"></script>\n</body>\n</html>\n");
        html
    }

    /// The site root: one link per locale.
    fn picker(&self) -> String {
        let ui = self.ui;
        let mut body = String::from("<h1>rustlings-production</h1>\n");
        let _ = writeln!(
            body,
            "<p>{}</p>\n<ul class=\"cards\">",
            render::escape(ui.text("site.picker", DEFAULT_PICKER))
        );
        for locale in self.locales {
            let _ = writeln!(
                body,
                "<li><a href=\"{}/index.html\">{}</a><span class=\"meta\">{}</span></li>",
                locale.as_str(),
                render::escape(&language_name(locale)),
                locale.as_str()
            );
        }
        body.push_str("</ul>\n");
        self.layout(&body)
    }

    /// A locale's home page: the welcome text and the tracks.
    fn home(&self, course: &Course, source: &Locale) -> String {
        let ui = self.ui;
        let manifest = course.manifest();
        let mut body = String::from("<h1>rustlings-production</h1>\n");
        let welcome = ui.welcome(manifest);
        // The note is for a reader whose locale has no text of its own — not for the source page.
        if !ui.has("welcome") && self.locale != Some(source) {
            let _ = writeln!(
                body,
                "<p class=\"meta\">{}</p>",
                render::escape(ui.text("site.source_note", DEFAULT_SOURCE_NOTE))
            );
        }
        body.push_str(&paragraphs(welcome));
        let _ = writeln!(
            body,
            "<h2>{}</h2>\n<ul class=\"cards\">",
            render::escape(ui.text("site.tracks", DEFAULT_TRACKS))
        );
        for track in course.tracks() {
            let steps = track.step.len();
            let title = self.locale.map_or(track.title.as_str(), |locale| {
                track.title_in(locale.as_str())
            });
            let _ = writeln!(
                body,
                "<li><a href=\"{}/index.html\">{}</a><span class=\"meta\">T{} · {steps} \
                 {} · {} <code>{}</code></span></li>",
                track.id,
                render::escape(title),
                track.index,
                render::escape(ui.text("site.unit", DEFAULT_UNIT)),
                render::escape(ui.text("site.project", DEFAULT_PROJECT)),
                render::escape(&track.project)
            );
        }
        body.push_str("</ul>\n");
        self.layout(&body)
    }

    /// A track's page: every step, in order, with the title the reader's locale gives it.
    fn overview(&self, track: &Track, locale: &Locale, source: &Locale, course: &Course) -> String {
        let ui = self.ui;
        let title = track.title_in(locale.as_str());
        let mut body = format!("<h1>{}</h1>\n", render::escape(title));
        let _ = writeln!(
            body,
            "<p class=\"meta\">T{} · {} {} · {} <code>{}</code></p>",
            track.index,
            track.step.len(),
            render::escape(ui.text("site.unit", DEFAULT_UNIT)),
            render::escape(ui.text("site.project", DEFAULT_PROJECT)),
            render::escape(&track.project)
        );
        body.push_str("<ol class=\"steps\">\n");
        for step in &track.step {
            let title = match step_body(course, step, locale, source) {
                Ok(body) => body_title(&body, step),
                Err(_) => step.title.clone(),
            };
            let _ = writeln!(
                body,
                "<li><a href=\"{}.html\">{}</a><span class=\"meta\">{}</span></li>",
                step.id,
                render::escape(&title),
                render::escape(&time_hint(ui, &step.time_hint))
            );
        }
        body.push_str("</ol>\n");
        self.layout(&body)
    }

    /// A step page: the body, a breadcrumb, and the neighbours.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &self,
        track: &Track,
        step: &Step,
        body: &i18n::StepBody,
        locale: &Locale,
        source: &Locale,
        previous: Option<&Step>,
        next: Option<&Step>,
    ) -> String {
        let ui = self.ui;
        let mut html = String::new();
        let _ = writeln!(
            html,
            "<nav class=\"crumbs\"><a href=\"index.html\">{}</a> / {}</nav>",
            render::escape(track.title_in(locale.as_str())),
            render::escape(&step.id)
        );
        if body.front_matter.locale.as_deref() != Some(locale.as_str()) && *locale != *source {
            let found = body
                .front_matter
                .locale
                .as_deref()
                .unwrap_or(source.as_str());
            let sentence = ui
                .text("site.fallback", DEFAULT_FALLBACK)
                .replace("{locale}", &render::escape(locale.as_str()))
                .replace("{found}", &render::escape(found));
            let _ = writeln!(html, "<p class=\"fallback\">{sentence}</p>");
        }
        html.push_str("<article>\n");
        html.push_str(&render::markdown(&body.text));
        html.push_str("</article>\n<nav class=\"pager\">");
        match previous {
            Some(before) => {
                let _ = write!(
                    html,
                    "<a class=\"prev\" href=\"{}.html\">← {}</a>",
                    before.id,
                    render::escape(&before.number)
                );
            }
            None => html.push_str("<span></span>"),
        }
        match next {
            Some(after) => {
                let _ = write!(
                    html,
                    "<a class=\"next\" href=\"{}.html\">{} →</a>",
                    after.id,
                    render::escape(&after.number)
                );
            }
            None => html.push_str("<span></span>"),
        }
        html.push_str("</nav>\n");
        self.layout(&html)
    }
}

/// `60m` in the reader's language: `60 分钟` or `60 min`. Anything that is not `<number>m` is
/// shown as authored — the hint is metadata, and the site does not invent a unit for it.
fn time_hint(ui: &Ui, hint: &str) -> String {
    let Some(minutes) = hint.strip_suffix('m') else {
        return hint.to_owned();
    };
    if minutes.is_empty() || !minutes.bytes().all(|byte| byte.is_ascii_digit()) {
        return hint.to_owned();
    }
    format!("{minutes} {}", ui.text("site.minutes", DEFAULT_MINUTES))
}

/// Turn a plain-text manifest block into paragraphs, with its `code spans` marked up.
fn paragraphs(text: &str) -> String {
    let mut html = String::new();
    let mut block: Vec<&str> = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !block.is_empty() {
                let _ = writeln!(html, "<p>{}</p>", render::inline(&block.join(" ")));
                block.clear();
            }
            continue;
        }
        block.push(line.trim());
    }
    html
}

/// A scratch directory unique to this process, removed by the caller.
fn scratch_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    std::env::temp_dir().join(format!("kiln-site-{}-{nanos}", std::process::id()))
}

/// Every internal `href`/`src` in the generated pages has to name a file that was written.
///
/// `kiln links` judges the *markdown*: it knows what a link to `2.15-shutdown-leases.md` means in
/// the repository, and nothing about the site, where the same link has to point at `.html`.
/// Rendering that rewrite wrong is exactly the mistake nobody notices until a reader clicks it, so
/// the check runs on the output — the one place where both the rewrite and the page names exist.
fn check_links(out: &Path) -> Result<()> {
    let mut broken: Vec<String> = Vec::new();
    for page in html_files(out)? {
        let html = std::fs::read_to_string(&page).map_err(|error| KilnError::Read {
            path: page.clone(),
            source: error,
        })?;
        let directory = page.parent().unwrap_or(out);
        for target in internal_targets(&html) {
            if !directory.join(&target).is_file() {
                broken.push(format!("{} → {target}", page.display()));
            }
        }
    }
    if broken.is_empty() {
        return Ok(());
    }
    broken.sort();
    Err(KilnError::Invalid(format!(
        "{} link(s) in the site point at files that were not written:\n  {}",
        broken.len(),
        broken.join("\n  ")
    )))
}

/// Every `.html` file under `out`, in a stable order.
fn html_files(out: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![out.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| KilnError::Read {
            path: directory.clone(),
            source: error,
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| KilnError::Read {
                path: directory.clone(),
                source: error,
            })?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "html")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Relative targets of `href="…"` and `src="…"` that are this site's own files.
fn internal_targets(html: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for attribute in ["href=\"", "src=\""] {
        let mut rest = html;
        while let Some(position) = rest.find(attribute) {
            let after = &rest[position + attribute.len()..];
            let Some(end) = after.find('"') else {
                break;
            };
            let target = &after[..end];
            rest = &after[end..];
            if target.starts_with('#')
                || target.contains("://")
                || target.starts_with("mailto:")
                || target.starts_with('/')
            {
                continue;
            }
            let path = target.split_once('#').map_or(target, |(path, _)| path);
            if !path.is_empty() {
                targets.push(path.to_owned());
            }
        }
    }
    targets
}

/// Write a file, creating its parent directories.
fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| KilnError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, content).map_err(|source| KilnError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// The stylesheet the site ships: readable at 46rem, dark when the reader's system is dark.
const STYLE: &str = r#":root {
  --ink: #1c1c1e; --muted: #5f6368; --paper: #fdfcf9; --rule: #e3e0d8; --accent: #7a4b12;
}
@media (prefers-color-scheme: dark) {
  :root { --ink: #e8e6e1; --muted: #9aa0a6; --paper: #17181a; --rule: #2c2e33; --accent: #e0a458; }
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--paper); color: var(--ink);
  font: 17px/1.65 -apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans SC", sans-serif;
}
header {
  display: flex; flex-wrap: wrap; gap: 1rem; align-items: center;
  padding: .8rem 1.2rem; border-bottom: 1px solid var(--rule);
  position: sticky; top: 0; background: var(--paper);
}
.brand { font-weight: 700; color: var(--ink); text-decoration: none; }
.langs { display: flex; gap: .6rem; font-size: .9rem; }
.langs a { color: var(--accent); }
.langs span { color: var(--muted); font-weight: 600; }
.search { margin-left: auto; position: relative; }
.search input {
  width: 13rem; padding: .35rem .6rem; border: 1px solid var(--rule);
  border-radius: .4rem; background: transparent; color: var(--ink); font: inherit; font-size: .9rem;
}
#results {
  position: absolute; right: 0; top: 2.2rem; width: 24rem; max-height: 60vh; overflow: auto;
  margin: 0; padding: .4rem; list-style: none; border: 1px solid var(--rule);
  border-radius: .4rem; background: var(--paper); box-shadow: 0 8px 24px rgba(0,0,0,.12); z-index: 10;
}
#results li a { display: block; padding: .3rem .4rem; text-decoration: none; color: var(--ink); }
#results li a:hover { background: var(--rule); border-radius: .3rem; }
#results small { display: block; color: var(--muted); }
main { max-width: 46rem; margin: 0 auto; padding: 1.5rem 1.2rem 4rem; }
h1, h2, h3 { line-height: 1.25; }
h1 { font-size: 1.9rem; margin: 1rem 0 .6rem; }
h2 { font-size: 1.3rem; margin: 2.2rem 0 .6rem; border-bottom: 1px solid var(--rule); padding-bottom: .25rem; }
h3 { font-size: 1.05rem; margin: 1.6rem 0 .4rem; }
a { color: var(--accent); }
/* Inline-block, so a code span is never split across two lines: an identifier stays whole, and no
   engine is left painting the empty leading fragment as a stray bar at the end of a line. Long
   spans wrap inside the box (capped at the column width) instead of overflowing. */
code {
  display: inline-block; max-width: 100%; overflow-wrap: break-word;
  font: .92em/1.4 ui-monospace, "SF Mono", Menlo, monospace;
  background: var(--rule); padding: .1em .3em; border-radius: .25rem;
}
pre code { display: inline; background: none; padding: 0; }
pre { background: var(--rule); padding: .9rem 1rem; border-radius: .5rem; overflow: auto; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; font-size: .95rem; }
.table-wrap { overflow-x: auto; margin: 1rem 0; }
.table-wrap table { margin: 0; }
th, td { border: 1px solid var(--rule); padding: .4rem .6rem; text-align: left; vertical-align: top; }
blockquote { margin: 1rem 0; padding: .2rem 1rem; border-left: 3px solid var(--rule); color: var(--muted); }
.meta { color: var(--muted); font-size: .85rem; }
.crumbs { color: var(--muted); font-size: .9rem; margin-bottom: .4rem; }
.fallback { padding: .6rem .8rem; border: 1px solid var(--rule); border-radius: .4rem; color: var(--muted); }
.cards, .steps { list-style: none; padding: 0; }
.cards li, .steps li { border: 1px solid var(--rule); border-radius: .5rem; padding: .7rem .9rem; margin: .5rem 0; }
.cards a, .steps a { text-decoration: none; font-weight: 600; }
.cards .meta, .steps .meta { display: block; font-weight: 400; }
.steps { counter-reset: step; }
.pager { display: flex; justify-content: space-between; margin-top: 3rem; padding-top: 1rem; border-top: 1px solid var(--rule); }
footer { text-align: center; color: var(--muted); font-size: .85rem; padding-bottom: 3rem; }
@media (max-width: 42rem) {
  /* The header is two rows on a phone; sticky costs a quarter of the screen, so it scrolls. */
  header { position: static; padding: .6rem .8rem; gap: .45rem; }
  .brand { font-size: .95rem; }
  .search { margin-left: 0; width: 100%; }
  .search input { width: 100%; }
  #results { right: auto; left: 0; width: 100%; }
  main { padding: 1rem .8rem 3rem; }
  main ul, main ol { padding-left: 1.2rem; }
  /* A phone cannot fit four columns: keep their natural width and let the wrapper scroll. */
  .table-wrap table { width: max-content; min-width: 100%; }
  h1 { font-size: 1.55rem; }
  .cards li, .steps li { padding: .55rem .7rem; }
}
"#;

/// The twenty lines of search: fetch the index once, filter on every keystroke.
const SEARCH: &str = r#"const box = document.getElementById("q");
const list = document.getElementById("results");
// A relative dataset value is not a valid base for `new URL`, so resolve it once against the page.
const base = new URL(box.dataset.index, window.location.href);
let index = null;
async function load() {
  if (index === null) {
    const response = await fetch(base);
    index = await response.json();
  }
  return index;
}
function show(rows) {
  list.textContent = "";
  if (rows.length === 0) { list.hidden = true; return; }
  for (const row of rows) {
    const item = document.createElement("li");
    const link = document.createElement("a");
    link.href = new URL(row.url, base).href;
    link.textContent = row.title;
    const small = document.createElement("small");
    small.textContent = row.step;
    link.append(small);
    item.append(link);
    list.append(item);
  }
  list.hidden = false;
}
box.addEventListener("input", async () => {
  const query = box.value.trim().toLowerCase();
  if (query.length < 2) { list.hidden = true; return; }
  const rows = (await load()).filter((row) =>
    (row.title + " " + row.text + " " + row.step).toLowerCase().includes(query));
  show(rows.slice(0, 12));
});
box.addEventListener("keydown", (event) => {
  if (event.key === "Escape") { box.value = ""; list.hidden = true; }
});
document.addEventListener("click", (event) => {
  if (!event.target.closest(".search")) { list.hidden = true; }
});
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn course(temp: &TempDir) -> Course {
        let course = crate::test_support::load_course(
            temp.path(),
            &crate::test_support::one_track(
                &[
                    crate::test_support::step_toml(
                        "mailer",
                        "1.1-workspace",
                        &["repo/workspace-layout@apply"],
                    ),
                    crate::test_support::step_toml(
                        "mailer",
                        "1.2-toolchain",
                        &["build/toolchain@apply"],
                    ),
                ],
                &["repo/workspace-layout@apply", "build/toolchain@apply"],
            ),
            &crate::test_support::ledger_toml(&[
                ("repo/workspace-layout", "apply", "mailer/1.1"),
                ("build/toolchain", "apply", "mailer/1.2"),
            ]),
        );
        temp.write(
            "course/docs/steps/en/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\ntitle = \"Split the crate\"\nlocale = \"en\"\n+++\n\n\
             # Split the crate\n\n## WHY\n\nBecause.\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
        );
        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\ntitle = \"Pin the toolchain\"\nlocale = \"en\"\n+++\n\n\
             # Pin the toolchain\n\n## WHY\n\nAlso because.\n",
        );
        temp.write(
            "course/docs/steps/zh-Hans/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\ntitle = \"拆分 crate\"\nlocale = \"zh-Hans\"\n\
             source_hash = \"sha256:00\"\ntranslator = \"t\"\n+++\n\n\
             # 拆分 crate\n\n## 为什么\n\n因为。\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
        );
        course
    }

    #[test]
    fn every_locale_renders_every_step_and_a_missing_translation_falls_back() {
        let temp = TempDir::new("site-build");
        let course = course(&temp);
        let out = temp.path().join("site");
        let report = build(&course, &Options::new(&out)).expect("the site builds");
        assert_eq!(
            report.locales,
            ["en", "zh-Hans"].map(|l| Locale::parse(l).unwrap())
        );
        // two locale homes + one track page and two steps each + the picker
        assert_eq!(report.pages, 2 + 2 * (1 + 2) + 1);
        let page = std::fs::read_to_string(out.join("zh-Hans/mailer/1.1-workspace.html")).unwrap();
        assert!(page.contains("拆分 crate"), "{page}");
        assert!(page.contains("<table>"), "the table renders: {page}");
        let fallback =
            std::fs::read_to_string(out.join("zh-Hans/mailer/1.2-toolchain.html")).unwrap();
        assert!(
            fallback.contains("no <code>zh-Hans</code> body"),
            "{fallback}"
        );
        assert!(fallback.contains("Pin the toolchain"), "{fallback}");
        let index = std::fs::read_to_string(out.join("search.json")).unwrap();
        assert!(index.contains("拆分 crate"), "{index}");
    }

    /// The catalogue is the course's one place for translated furniture (CLI and site alike).
    fn course_with_catalogue(temp: &TempDir) -> Course {
        let mut tracks = crate::test_support::one_track(
            &[
                crate::test_support::step_toml(
                    "mailer",
                    "1.1-workspace",
                    &["repo/workspace-layout@apply"],
                ),
                crate::test_support::step_toml(
                    "mailer",
                    "1.2-toolchain",
                    &["build/toolchain@apply"],
                ),
            ],
            &["repo/workspace-layout@apply", "build/toolchain@apply"],
        );
        tracks[0].2 = tracks[0].2.replacen(
            "[[step]]",
            "[localized_titles]\nzh-Hans = \"邮件服务\"\n\n[[step]]",
            1,
        );
        let course = crate::test_support::load_course(
            temp.path(),
            &tracks,
            &crate::test_support::ledger_toml(&[
                ("repo/workspace-layout", "apply", "mailer/1.1"),
                ("build/toolchain", "apply", "mailer/1.2"),
            ]),
        );
        temp.write(
            "course/docs/steps/en/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\ntitle = \"Split\"\nlocale = \"en\"\n+++\n\n\
             # Split\n\n## WHY\n\nBecause.\n",
        );
        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\ntitle = \"Pin\"\nlocale = \"en\"\n+++\n\n\
             # Pin\n\n## WHY\n\nAlso.\n",
        );
        temp.write(
            "course/docs/steps/zh-Hans/mailer/1.1-workspace.md",
            "+++\nid = \"mailer/1.1-workspace\"\ntitle = \"拆分\"\nlocale = \"zh-Hans\"\n\
             source_hash = \"sha256:00\"\ntranslator = \"t\"\n+++\n\n# 拆分\n\n## 为什么\n\n因为。\n",
        );
        temp.write(
            "course/locales/zh-Hans/ui.toml",
            "locale = \"zh-Hans\"\n\n[string]\nwelcome = \"欢迎，按步骤来。\"\n\
             \"site.tracks\" = \"课程\"\n\"site.unit\" = \"步\"\n\"site.project\" = \"项目\"\n\
             \"site.minutes\" = \"分钟\"\n\"site.source_note\" = \"英文原文\"\n",
        );
        course
    }

    #[test]
    fn a_catalogue_translates_the_home_page_and_the_track_name() {
        let temp = TempDir::new("site-catalogue");
        let course = course_with_catalogue(&temp);
        let out = temp.path().join("site");
        build(&course, &Options::new(&out)).expect("the site builds");
        let home = std::fs::read_to_string(out.join("zh-Hans/index.html")).unwrap();
        assert!(home.contains("欢迎，按步骤来。"), "{home}");
        assert!(home.contains(">课程<"), "{home}");
        assert!(home.contains("邮件服务"), "{home}");
        assert!(!home.contains("<details>"), "no disclosure widget: {home}");
        let track = std::fs::read_to_string(out.join("zh-Hans/mailer/index.html")).unwrap();
        assert!(track.contains("邮件服务"), "{track}");
        assert!(track.contains("2 步"), "{track}");
        assert!(track.contains("60 分钟"), "{track}");
    }

    #[test]
    fn without_a_catalogue_the_home_page_says_the_text_is_in_english() {
        let temp = TempDir::new("site-no-catalogue");
        let course = course(&temp);
        let out = temp.path().join("site");
        build(&course, &Options::new(&out)).expect("the site builds");
        let home = std::fs::read_to_string(out.join("zh-Hans/index.html")).unwrap();
        assert!(home.contains("in English — the only locale"), "{home}");
        // The source page is not a translation of anything, so it carries no note.
        let source = std::fs::read_to_string(out.join("en/index.html")).unwrap();
        assert!(!source.contains("in English — the only locale"), "{source}");
    }

    #[test]
    fn a_step_with_no_body_anywhere_is_an_error_not_a_blank_page() {
        let temp = TempDir::new("site-missing");
        let course = course(&temp);
        std::fs::remove_file(
            temp.path()
                .join("course/docs/steps/en/mailer/1.2-toolchain.md"),
        )
        .unwrap();
        let error = build(&course, &Options::new(temp.path().join("site"))).unwrap_err();
        assert!(error.to_string().contains("1.2-toolchain"), "{error}");
    }

    #[test]
    fn check_mode_leaves_nothing_behind() {
        let temp = TempDir::new("site-check");
        let course = course(&temp);
        let report = build(&course, &Options::check()).expect("the site renders");
        assert!(report.pages > 0);
        assert!(!report.out.exists(), "the scratch directory is removed");
    }

    #[test]
    fn a_body_line_that_starts_with_a_tag_is_refused() {
        let temp = TempDir::new("site-tag-line");
        let course = course(&temp);
        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\ntitle = \"Pin\"\nlocale = \"en\"\n+++\n\n\
             # Pin\n\npromises: `backup --store\n<DIR> --to <DIR>`, and rest.\n",
        );
        let error = build(&course, &Options::new(temp.path().join("site"))).unwrap_err();
        assert!(
            error.to_string().contains("may not start with `<`"),
            "{error}"
        );
    }

    #[test]
    fn the_check_catches_a_link_no_page_answers() {
        let temp = TempDir::new("site-broken-link");
        let out = temp.path().join("site");
        write_file(
            &out.join("index.html"),
            "<a href=\"missing.html\">x</a><a href=\"https://example.com\">fine</a>",
        )
        .unwrap();
        let error = check_links(&out).unwrap_err();
        assert!(error.to_string().contains("missing.html"), "{error}");
        assert!(!error.to_string().contains("example.com"), "{error}");
    }

    #[test]
    fn a_page_that_links_a_sibling_step_renders_a_link_the_check_accepts() {
        let temp = TempDir::new("site-sibling");
        let course = course(&temp);
        temp.write(
            "course/docs/steps/en/mailer/1.2-toolchain.md",
            "+++\nid = \"mailer/1.2-toolchain\"\ntitle = \"Pin\"\nlocale = \"en\"\n+++\n\n\
             # Pin\n\nSee [the split](1.1-workspace.md#why).\n",
        );
        let out = temp.path().join("site");
        let report = build(&course, &Options::new(&out)).expect("links resolve");
        assert!(report.pages > 0);
        let page = std::fs::read_to_string(out.join("en/mailer/1.2-toolchain.html")).unwrap();
        assert!(page.contains("href=\"1.1-workspace.html#why\""), "{page}");
    }

    #[test]
    fn internal_targets_ignore_what_is_not_a_page_of_this_site() {
        let html = "<a href=\"#top\">t</a><a href=\"https://x/y\">e</a>\
                    <a href=\"mailto:a@b\">m</a><a href=\"/abs\">a</a><a href=\"page.html\">p</a>";
        assert_eq!(internal_targets(html), ["page.html"]);
    }

    #[test]
    fn the_excerpt_is_prose_without_tables_or_code() {
        let text = "# Title\n\nSome words here.\n\n```sh\nkiln run\n```\n\n| a | b |\n| - | - |\n";
        let excerpt = excerpt(text);
        assert_eq!(excerpt, "Some words here.");
    }
}
