//! Markdown → HTML, and the one rewrite the site needs.
//!
//! The course pages are CommonMark plus GFM tables, nothing else (surveyed across all 238 bodies:
//! no images, no footnotes, no raw HTML). `pulldown-cmark` is a pull parser for exactly that; the
//! only thing added on top is a link rewrite, because a step body may point at a sibling page by
//! its repository name (`[2.15](2.15-shutdown-leases.md)`) and the site's sibling is `.html`.

use pulldown_cmark::{Options, Parser, html};

/// Render one step body (front matter already stripped) as an HTML fragment.
#[must_use]
pub fn markdown(text: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(text, options);
    let mut out = String::with_capacity(text.len());
    html::push_html(&mut out, parser);
    rewrite_links(&out)
}

/// Escape text for an HTML text node or a double-quoted attribute.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Point every `href` that names a sibling markdown page at the HTML page instead.
fn rewrite_links(markdown_html: &str) -> String {
    let mut out = String::with_capacity(markdown_html.len());
    let mut rest = markdown_html;
    while let Some(position) = rest.find("href=\"") {
        let (before, after) = rest.split_at(position + "href=\"".len());
        out.push_str(before);
        let Some(end) = after.find('"') else {
            out.push_str(after);
            return out;
        };
        let (target, tail) = after.split_at(end);
        out.push_str(&site_href(target));
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// `2.15-shutdown-leases.md#leases` → `2.15-shutdown-leases.html#leases`.
///
/// Only repository-relative targets are rewritten: an absolute URL that happens to end in `.md`
/// (a crate's documentation, say) is somebody else's file and stays exactly as written.
fn site_href(target: &str) -> String {
    if target.starts_with('/') || target.contains("://") || target.starts_with("mailto:") {
        return target.to_owned();
    }
    let (path, fragment) = match target.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (target, None),
    };
    let Some(stem) = path.strip_suffix(".md") else {
        return target.to_owned();
    };
    let mut out = String::with_capacity(stem.len() + 4);
    out.push_str(stem);
    out.push_str(".html");
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_becomes_a_table() {
        let html = markdown("| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("<td>1</td>"), "{html}");
    }

    #[test]
    fn a_fenced_block_keeps_its_language_and_its_text() {
        let html = markdown("```sh\nkiln run mailer/1.1\n```\n");
        assert!(
            html.contains("<code class=\"language-sh\">kiln run mailer/1.1"),
            "{html}"
        );
    }

    #[test]
    fn a_sibling_markdown_link_becomes_an_html_link() {
        let html = markdown("[2.15](2.15-shutdown-leases.md#leases)");
        assert!(
            html.contains("href=\"2.15-shutdown-leases.html#leases\""),
            "{html}"
        );
    }

    #[test]
    fn a_link_that_is_not_a_repository_page_is_left_alone() {
        for target in [
            "https://docs.rs/pulldown-cmark/latest/readme.md",
            "#a-heading",
            "mailto:someone@example.com",
            "/absolute/path.md",
        ] {
            let html = markdown(&format!("[x]({target})"));
            assert!(
                html.contains(&format!("href=\"{target}\"")),
                "{target}: {html}"
            );
        }
    }

    #[test]
    fn escaping_covers_the_five_characters_that_matter() {
        assert_eq!(escape("a<b&c>\"d'e"), "a&lt;b&amp;c&gt;&quot;d&#39;e");
    }
}
