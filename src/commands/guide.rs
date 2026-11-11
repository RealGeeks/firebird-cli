//! `firebird-cli guide [TOPIC]` — the embedded guide, whole or one section
//! at a time so an agent can load only what it needs.

use clap::Args;

use crate::error::{Error, Result};

#[derive(Debug, Args)]
pub struct GuideArgs {
    /// A section number or a word from its title, e.g. `drafts`, `blocks`, `undo`, `12`
    #[arg(value_name = "TOPIC")]
    pub topic: Option<String>,
    /// List the section titles
    #[arg(long)]
    pub list: bool,
}

pub struct Section<'a> {
    pub number: usize,
    pub title: &'a str,
    pub body: &'a str,
}

/// Words agents are likely to reach for, and the title text they stand for.
const ALIASES: &[(&str, &str)] = &[
    ("auth", "setup"),
    ("login", "setup"),
    ("key", "setup"),
    ("install", "setup"),
    ("json", "output"),
    ("errors", "exit codes"),
    ("exit", "exit codes"),
    ("publish", "drafts"),
    ("draft", "drafts"),
    ("live", "drafts"),
    ("schema", "editing blocks"),
    ("patch", "editing blocks"),
    ("nav", "site settings"),
    ("navigation", "site settings"),
    ("theme", "site settings"),
    ("styling", "site settings"),
    ("footer", "site settings"),
    ("header", "site settings"),
    ("settings", "site settings"),
    ("revert", "undo"),
    ("restore", "undo"),
    ("history", "undo"),
    ("revisions", "undo"),
    ("versions", "undo"),
    ("media", "images"),
    ("upload", "images"),
    ("fair housing", "writing content"),
    ("api", "anything else"),
    ("raw", "anything else"),
];

/// Split the guide at its `## N. Title` headings.
pub fn sections(guide: &str) -> Vec<Section<'_>> {
    let mut out = Vec::new();
    let mut starts: Vec<(usize, usize, &str)> = Vec::new();
    for (offset, line) in line_offsets(guide) {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some((num, title)) = rest.split_once(". ") {
                if let Ok(number) = num.trim().parse::<usize>() {
                    starts.push((offset, number, title.trim()));
                }
            }
        }
    }
    for (i, (offset, number, title)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(guide.len(), |s| s.0);
        out.push(Section {
            number: *number,
            title,
            body: guide[*offset..end].trim_end(),
        });
    }
    out
}

fn line_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split_inclusive('\n').map(move |line| {
        let start = offset;
        offset += line.len();
        (start, line.trim_end_matches('\n'))
    })
}

/// Sections matching a topic: by number, else by a case-insensitive word
/// in the title (or one of the [`ALIASES`]).
pub fn find(all: &[Section<'_>], topic: &str) -> Vec<usize> {
    let wanted = topic.trim().to_ascii_lowercase();
    if let Ok(n) = wanted.parse::<usize>() {
        return all
            .iter()
            .enumerate()
            .filter(|(_, s)| s.number == n)
            .map(|(i, _)| i)
            .collect();
    }
    let needle = ALIASES
        .iter()
        .find(|(k, _)| *k == wanted)
        .map_or(wanted.as_str(), |(_, v)| v);
    all.iter()
        .enumerate()
        .filter(|(_, s)| s.title.to_ascii_lowercase().contains(needle))
        .map(|(i, _)| i)
        .collect()
}

pub fn run(args: &GuideArgs) -> Result<()> {
    let all = sections(crate::GUIDE);
    if args.list {
        for s in &all {
            println!("{:>2}  {}", s.number, s.title);
        }
        return Ok(());
    }
    let Some(topic) = &args.topic else {
        print!("{}", crate::GUIDE);
        return Ok(());
    };
    let hits = find(&all, topic);
    if hits.is_empty() {
        let titles: Vec<String> = all
            .iter()
            .map(|s| format!("{}. {}", s.number, s.title))
            .collect();
        return Err(Error::Usage(format!(
            "no guide section matches '{topic}'. Sections:\n  {}",
            titles.join("\n  ")
        )));
    }
    for (n, i) in hits.iter().enumerate() {
        if n > 0 {
            println!();
        }
        println!("{}", all[*i].body);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Guide\n\nintro\n\n## 1. Setup\n\nlogin here\n\n## 2. Drafts, publishing and what is live\n\nrules\n\n## 3. Exit codes\n\ncodes\n";

    #[test]
    fn splits_sections() {
        let all = sections(DOC);
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].title, "Drafts, publishing and what is live");
        assert!(all[1].body.starts_with("## 2.") && all[1].body.ends_with("rules"));
    }

    #[test]
    fn finds_by_number_word_and_alias() {
        let all = sections(DOC);
        assert_eq!(find(&all, "3"), vec![2]);
        assert_eq!(find(&all, "Publishing"), vec![1]);
        assert_eq!(find(&all, "draft"), vec![1]);
        assert_eq!(find(&all, "errors"), vec![2]);
        assert_eq!(find(&all, "nope"), Vec::<usize>::new());
    }

    #[test]
    fn real_guide_sections_are_numbered_in_order() {
        let all = sections(crate::GUIDE);
        assert!(all.len() >= 10);
        for (i, section) in all.iter().enumerate() {
            assert_eq!(section.number, i + 1, "{}", section.title);
        }
    }

    #[test]
    fn real_guide_has_the_expected_topics() {
        let all = sections(crate::GUIDE);
        for topic in [
            "setup", "output", "start", "drafts", "blocks", "settings", "undo", "images",
            "writing", "exit",
        ] {
            assert!(!find(&all, topic).is_empty(), "no section for {topic}");
        }
    }

    #[test]
    fn every_alias_leads_to_a_section() {
        let all = sections(crate::GUIDE);
        for (alias, _) in ALIASES {
            assert!(!find(&all, alias).is_empty(), "no section for {alias}");
        }
    }
}
