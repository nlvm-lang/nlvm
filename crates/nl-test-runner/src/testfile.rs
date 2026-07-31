use anyhow::{Context, Result};

use crate::header::Header;
use nl_test_runner::fixture::{parse_blocks, split_front_matter, SourceBlock};

pub struct TestFile {
    pub header: Header,
    pub blocks: Vec<SourceBlock>,
}

pub fn parse_test_file(content: &str) -> Result<TestFile> {
    let (yaml_str, body_lines) = split_front_matter(content)?;

    let header: Header = if yaml_str.trim().is_empty() {
        Header::default()
    } else {
        serde_yaml::from_str(&yaml_str).context("parsing YAML front matter")?
    };

    let separator = header.file_separator_or_default().to_string();
    let blocks = parse_blocks(&body_lines, &separator);

    Ok(TestFile { header, blocks })
}
