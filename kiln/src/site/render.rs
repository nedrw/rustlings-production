//! Markdown → HTML, and the one rewrite the site needs.
//!
//! The course pages are CommonMark plus GFM tables, nothing else (surveyed across every body: no
//! images, no footnotes, no raw HTML). `pulldown-cmark` is a pull parser for exactly that; the only
//! thing added on top is a link rewrite, because a step body may point at a sibling page by its
//! repository name (`[2.15](2.15-shutdown-leases.md)`) and the site's sibling is `.html`.
//!
//! The rewrite — and the link check that guards it — only ever looks at *markup*, never at the
//! text inside `<code>`/`<pre>`: an HTML example that prints `href="page.md"` is content, and
//! changing it would teach the wrong bytes.

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
    wrap_tables(&rewrite_links(&out))
}

/// Render a plain-text block (the manifest's welcome): escape it, and turn `code spans` into
/// `<code>`.
///
/// Not `markdown()`: the welcome is written for a terminal, where `app/<track>` is literal text.
/// Through the markdown parser `<track>` would become an HTML element and disappear, so this path
/// escapes first and only then recognises the one piece of markup the text actually uses.
#[must_use]
pub fn inline(text: &str) -> String {
    let escaped = escape(text);
    // A lone backtick is a typo, not an unterminated span: leave the text alone rather than open a
    // tag that can never close (and that would restyle the rest of the paragraph).
    if !escaped.matches('`').count().is_multiple_of(2) {
        return escaped;
    }
    let mut out = String::with_capacity(escaped.len());
    let mut inside = false;
    for character in escaped.chars() {
        if character == '`' {
            out.push_str(if inside { "</code>" } else { "<code>" });
            inside = !inside;
        } else {
            out.push(character);
        }
    }
    out
}

/// A wide table scrolls instead of squeezing its columns on a narrow screen.
fn wrap_tables(html: &str) -> String {
    html.replace("<table>", "<div class=\"table-wrap\"><table>")
        .replace("</table>", "</table></div>")
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

/// `(start, end)` byte ranges of every `href`/`src` attribute value outside `<code>`/`<pre>`.
///
/// Text inside those two elements is a sample, not a link: the site quotes HTTP and HTML, and a
/// sample that says `href="page.md"` must keep saying it.
fn link_ranges(html: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut depth = 0usize;
    let mut cursor = 0usize;
    let mut index = 0usize;
    while let Some(offset) = html[index..].find('<') {
        let tag_start = index + offset;
        let Some(length) = html[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + length + 1;
        let tag = &html[tag_start..tag_end];
        let name = tag[1..]
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .next()
            .unwrap_or_default();
        if name.eq_ignore_ascii_case("code") || name.eq_ignore_ascii_case("pre") {
            if depth == 0 && tag_start > cursor {
                collect(&html[cursor..tag_start], cursor, &mut ranges);
            }
            if tag.starts_with("</") {
                depth = depth.saturating_sub(1);
            } else {
                depth += 1;
            }
            cursor = tag_end;
        }
        index = tag_end;
    }
    if depth == 0 && cursor < html.len() {
        collect(&html[cursor..], cursor, &mut ranges);
    }
    ranges
}

/// Attribute values in one stretch of markup, as absolute byte ranges.
fn collect(segment: &str, offset: usize, ranges: &mut Vec<(usize, usize)>) {
    for attribute in ["href=\"", "src=\""] {
        let mut rest = 0usize;
        while let Some(position) = segment[rest..].find(attribute) {
            let value_start = rest + position + attribute.len();
            let Some(length) = segment[value_start..].find('"') else {
                break;
            };
            ranges.push((offset + value_start, offset + value_start + length));
            rest = value_start + length;
        }
    }
    ranges.sort_unstable();
}

/// Every link target the site controls (outside code samples).
pub fn link_targets(html: &str) -> Vec<&str> {
    link_ranges(html)
        .into_iter()
        .map(|(start, end)| &html[start..end])
        .collect()
}

/// Point every `href` that names a sibling markdown page at the HTML page instead.
fn rewrite_links(html: &str) -> String {
    let ranges = link_ranges(html);
    if ranges.is_empty() {
        return html.to_owned();
    }
    let mut out = String::with_capacity(html.len());
    let mut last = 0usize;
    for (start, end) in ranges {
        out.push_str(&html[last..start]);
        out.push_str(&site_href(&html[start..end]));
        last = end;
    }
    out.push_str(&html[last..]);
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
