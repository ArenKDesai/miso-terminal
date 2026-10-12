//! The Markdown an assistant writes, drawn with egui: paragraphs, headings,
//! bullet and numbered lists, `**bold**`, `` `code` ``, fenced code blocks
//! and pipe tables. Anything else shows as written.
//!
//! A code span can be made clickable (ASK turns terminal commands into
//! links): [`show`] asks `link` about each one and returns the one clicked.

use egui::{Grid, RichText, Ui};

use crate::skin::Skin;

/// One piece of a line.
#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Text { text: String, bold: bool },
    Code(String),
}

/// A block of the document.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(Vec<Span>),
    Paragraph(Vec<Span>),
    /// A list item: its marker (`•` or `3.`) and text.
    Item(String, Vec<Span>),
    Code(String),
    Table(Vec<Vec<Vec<Span>>>),
    Gap,
}

/// Split inline Markdown into spans: `**` toggles bold, backticks enclose code.
pub fn spans(line: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut bold = false;
    let mut chars = line.chars().peekable();
    let flush = |text: &mut String, bold: bool, out: &mut Vec<Span>| {
        if !text.is_empty() {
            out.push(Span::Text {
                text: std::mem::take(text),
                bold,
            });
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                flush(&mut text, bold, &mut out);
                bold = !bold;
            }
            '`' => {
                let code: String = chars.by_ref().take_while(|c| *c != '`').collect();
                flush(&mut text, bold, &mut out);
                out.push(Span::Code(code));
            }
            c => text.push(c),
        }
    }
    flush(&mut text, bold, &mut out);
    out
}

/// Split a document into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !matches!(blocks.last(), None | Some(Block::Gap)) {
                blocks.push(Block::Gap);
            }
        } else if trimmed.starts_with("```") {
            let mut code = Vec::new();
            for l in lines.by_ref() {
                if l.trim().starts_with("```") {
                    break;
                }
                code.push(l);
            }
            blocks.push(Block::Code(code.join("\n")));
        } else if trimmed.starts_with('|') {
            let mut rows = vec![table_row(trimmed)];
            while let Some(next) = lines.peek().map(|l| l.trim()) {
                if !next.starts_with('|') {
                    break;
                }
                lines.next();
                // The `|---|---|` line under the header.
                if !next.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                    rows.push(table_row(next));
                }
            }
            blocks.push(Block::Table(rows));
        } else if let Some(h) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            blocks.push(Block::Heading(spans(h)));
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "))
        {
            let indent = line.len() - line.trim_start().len();
            let marker = if indent >= 2 { "◦" } else { "•" };
            blocks.push(Block::Item(marker.into(), spans(item)));
        } else if let Some((n, rest)) = trimmed.split_once(". ")
            && !n.is_empty()
            && n.len() <= 3
            && n.chars().all(|c| c.is_ascii_digit())
        {
            blocks.push(Block::Item(format!("{n}."), spans(rest)));
        } else {
            blocks.push(Block::Paragraph(spans(trimmed)));
        }
    }
    while matches!(blocks.last(), Some(Block::Gap)) {
        blocks.pop();
    }
    blocks
}

fn table_row(line: &str) -> Vec<Vec<Span>> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| spans(cell.trim()))
        .collect()
}

/// Draw `text`. `link` says whether a code span is a command to offer as a
/// link; the one clicked this frame is returned.
pub fn show(
    ui: &mut Ui,
    skin: &Skin,
    id: &str,
    text: &str,
    link: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut clicked = None;
    for (i, block) in parse(text).iter().enumerate() {
        match block {
            Block::Gap => ui.add_space(6.0),
            Block::Heading(s) => {
                ui.add_space(2.0);
                line(ui, skin, s, true, &link, &mut clicked);
            }
            Block::Paragraph(s) => line(ui, skin, s, false, &link, &mut clicked),
            Block::Item(marker, s) => {
                ui.horizontal_top(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(marker).color(skin.text_muted));
                    ui.vertical(|ui| line(ui, skin, s, false, &link, &mut clicked));
                });
            }
            Block::Code(code) => {
                egui::Frame::new()
                    .fill(skin.surface_alt)
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(code).monospace().color(skin.text));
                    });
            }
            Block::Table(rows) => {
                egui::ScrollArea::horizontal()
                    .id_salt(format!("{id}-table-{i}"))
                    .show(ui, |ui| {
                        Grid::new(format!("{id}-grid-{i}"))
                            .striped(true)
                            .spacing([14.0, 3.0])
                            .show(ui, |ui| {
                                for (r, row) in rows.iter().enumerate() {
                                    for cell in row {
                                        line(ui, skin, cell, r == 0, &link, &mut clicked);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            }
        }
    }
    clicked
}

/// One run of spans, wrapping word by word.
fn line(
    ui: &mut Ui,
    skin: &Skin,
    spans: &[Span],
    strong: bool,
    link: &impl Fn(&str) -> bool,
    clicked: &mut Option<String>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for span in spans {
            match span {
                Span::Text { text, bold } => {
                    for word in text.split_inclusive(' ') {
                        let mut t = RichText::new(word);
                        if *bold || strong {
                            t = t.strong().color(skin.text_strong);
                        }
                        ui.label(t);
                    }
                }
                Span::Code(code) if link(code) => {
                    if crate::widgets::link(ui, skin, code)
                        .on_hover_text("Open it in the terminal")
                        .clicked()
                    {
                        *clicked = Some(code.clone());
                    }
                }
                Span::Code(code) => {
                    ui.label(RichText::new(code).monospace().color(skin.accent));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Span {
        Span::Text {
            text: t.into(),
            bold: false,
        }
    }

    #[test]
    fn inline_spans() {
        assert_eq!(
            spans("RT hit **$10,000** at `GP INDIANA.HUB 1` today"),
            vec![
                text("RT hit "),
                Span::Text {
                    text: "$10,000".into(),
                    bold: true
                },
                text(" at "),
                Span::Code("GP INDIANA.HUB 1".into()),
                text(" today"),
            ]
        );
    }

    #[test]
    fn blocks() {
        let doc = "## Why\n\nEnergy set it.\n\n- one\n  - two\n3. three\n\n| Hub | LMP |\n|---|---:|\n| MINN | 9 |\n\n```\ncode\n```\n\n";
        let b = parse(doc);
        assert_eq!(b[0], Block::Heading(vec![text("Why")]));
        assert_eq!(b[1], Block::Gap);
        assert_eq!(b[2], Block::Paragraph(vec![text("Energy set it.")]));
        assert_eq!(b[4], Block::Item("•".into(), vec![text("one")]));
        assert_eq!(b[5], Block::Item("◦".into(), vec![text("two")]));
        assert_eq!(b[6], Block::Item("3.".into(), vec![text("three")]));
        assert_eq!(
            b[8],
            Block::Table(vec![
                vec![vec![text("Hub")], vec![text("LMP")]],
                vec![vec![text("MINN")], vec![text("9")]],
            ])
        );
        assert_eq!(b[10], Block::Code("code".into()));
        assert_eq!(b.len(), 11, "no trailing gap");
    }
}
