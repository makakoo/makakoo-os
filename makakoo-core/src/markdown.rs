//! Shared CommonMark parsing used by OKF validation and ingestion.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownLink {
    pub destination: String,
    pub source_range: Range<usize>,
}

/// Return actual CommonMark links with source offsets.
///
/// Parser events naturally exclude inline, fenced, indented, and raw-HTML
/// code while resolving full, collapsed, and shortcut reference links.
/// Images are deliberately excluded because OKF links assert relationships.
pub fn links(content: &str) -> Vec<MarkdownLink> {
    Parser::new_ext(content, Options::empty())
        .into_offset_iter()
        .filter_map(|(event, source_range)| match event {
            Event::Start(Tag::Link { dest_url, .. }) => Some(MarkdownLink {
                destination: dest_url.into_string(),
                source_range,
            }),
            _ => None,
        })
        .collect()
}

pub fn link_destinations(content: &str) -> Vec<String> {
    links(content)
        .into_iter()
        .map(|link| link.destination)
        .collect()
}

pub fn reference_definition_ranges(content: &str) -> Vec<Range<usize>> {
    Parser::new_ext(content, Options::empty())
        .reference_definitions()
        .iter()
        .map(|(_, definition)| definition.span.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_and_reference_links_but_not_literal_code_or_images() {
        let content = concat!(
            "[Inline](inline.md) and [Reference][ref] and [Shortcut].\n\n",
            "[ref]: reference.md \"Reference title\"\n",
            "[shortcut]: shortcut.md\n\n",
            "`[Inline code](inline-code.md)`\n\n",
            "    [Indented code](indented-code.md)\n\n",
            "```markdown\n[Fenced](fenced.md)\n```\n\n",
            "<pre>\n[HTML code](html-code.md)\n</pre>\n\n",
            "![Image](image.md)\n",
        );

        assert_eq!(
            link_destinations(content),
            vec!["inline.md", "reference.md", "shortcut.md"]
        );
        assert_eq!(reference_definition_ranges(content).len(), 2);
    }
}
