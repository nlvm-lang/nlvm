use anyhow::{bail, Context, Result};

/// One `#NLFILE <path>` block of a fixture: the source file it stands for and
/// its contents.
pub struct SourceBlock {
    pub path: String,
    pub content: String,
}

/// Splits a fixture into its `---`-delimited YAML front matter (returned as a
/// raw string, so each harness can deserialize its own header type) and the
/// body lines that follow it.
pub fn split_front_matter(content: &str) -> Result<(String, Vec<&str>)> {
    let mut lines = content.lines();

    let first = lines.next().context("empty fixture file")?;
    if first.trim() != "---" {
        bail!("fixture must start with '---' front matter delimiter");
    }

    let mut yaml_lines = Vec::new();
    let mut found_closing = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            found_closing = true;
            break;
        }
        yaml_lines.push(line);
    }
    if !found_closing {
        bail!("missing closing '---' for front matter");
    }

    Ok((yaml_lines.join("\n"), lines.collect()))
}

/// Splits the body into one `SourceBlock` per `<separator> <path>` line.
pub fn parse_blocks(body_lines: &[&str], separator: &str) -> Vec<SourceBlock> {
    let mut blocks = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_content = String::new();

    for line in body_lines {
        if let Some(rest) = line.strip_prefix(separator) {
            if let Some(path) = current_path.take() {
                blocks.push(SourceBlock {
                    path,
                    content: std::mem::take(&mut current_content),
                });
            }
            current_path = Some(rest.trim().to_string());
        } else if current_path.is_some() {
            current_content.push_str(line);
            current_content.push('\n');
        }
        // Lines before the first separator (e.g. a blank line) are ignored.
    }
    if let Some(path) = current_path.take() {
        blocks.push(SourceBlock {
            path,
            content: current_content,
        });
    }
    blocks
}
