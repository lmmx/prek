use std::path::Path;

use anyhow::Result;
use clap::Parser;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

const CONFLICT_PATTERNS: &[&[u8]] = &[
    b"<<<<<<< ",
    b"======= ",
    b"=======\r\n",
    b"=======\n",
    b">>>>>>> ",
];

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long)]
    assume_in_merge: bool,
}

pub(crate) async fn check_merge_conflict(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    // Check if we're in a merge state or assuming merge
    if !args.assume_in_merge && !is_in_merge(hook.work_dir()).await? {
        return Ok((0, Vec::new()));
    }

    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| check_file(hook.project().relative_path(), filename).await)
        .buffered(*CONCURRENCY);

    let mut code = 0;
    let mut output = Vec::new();

    while let Some(result) = tasks.next().await {
        let (c, o) = result?;
        code |= c;
        output.extend(o);
    }

    Ok((code, output))
}

async fn is_in_merge(work_dir: &Path) -> Result<bool> {
    // Get the git directory
    let git_dir_output = tokio::process::Command::new("git")
        .arg("rev-parse")
        .arg("--git-dir")
        .current_dir(work_dir)
        .output()
        .await?;

    if !git_dir_output.status.success() {
        return Ok(false);
    }

    let git_dir = String::from_utf8_lossy(&git_dir_output.stdout)
        .trim()
        .to_string();
    let git_dir_path = if Path::new(&git_dir).is_absolute() {
        Path::new(&git_dir).to_path_buf()
    } else {
        work_dir.join(&git_dir)
    };

    // Check if MERGE_MSG exists
    let merge_msg_exists = git_dir_path.join("MERGE_MSG").exists();
    if !merge_msg_exists {
        return Ok(false);
    }

    // Check if any of the merge state files exist
    let merge_head_exists = git_dir_path.join("MERGE_HEAD").exists();
    let rebase_apply_exists = git_dir_path.join("rebase-apply").exists();
    let rebase_merge_exists = git_dir_path.join("rebase-merge").exists();

    Ok(merge_head_exists || rebase_apply_exists || rebase_merge_exists)
}

async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let content = fs_err::tokio::read(file_base.join(filename)).await?;

    let mut code = 0;
    let mut output = Vec::new();
    let mut line_number = 1;
    let mut i = 0;

    while i < content.len() {
        // Find the start of this line
        let line_start = i;

        // Find the end of this line (including line ending)
        let mut line_end = i;
        while line_end < content.len() && content[line_end] != b'\n' {
            line_end += 1;
        }
        if line_end < content.len() && content[line_end] == b'\n' {
            line_end += 1; // Include the \n
        }

        let line = &content[line_start..line_end];

        // Check all patterns
        for pattern in CONFLICT_PATTERNS {
            if line.starts_with(pattern) {
                // Don't trim the pattern - display it as-is (minus any line endings)
                let pattern_display = if pattern.ends_with(b"\r\n") {
                    &pattern[..pattern.len() - 2]
                } else if pattern.ends_with(b"\n") {
                    &pattern[..pattern.len() - 1]
                } else {
                    pattern
                };
                let pattern_str = String::from_utf8_lossy(pattern_display);
                let error_message = format!(
                    "{}:{}: Merge conflict string {:?} found\n",
                    filename.display(),
                    line_number,
                    pattern_str.as_ref()
                );
                output.extend(error_message.into_bytes());
                code = 1;
                break; // Only report one pattern per line
            }
        }

        line_number += 1;
        i = line_end;
    }

    Ok((code, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &[u8],
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_no_conflict_markers() -> Result<()> {
        let dir = tempdir()?;
        let content = b"This is a normal file\nWith no conflict markers\n";
        let file_path = create_test_file(&dir, "clean.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_start() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\n<<<<<<< HEAD\nConflicting line\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("<<<<<<< "));
        assert!(output_str.contains("conflict.txt:2"));
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_middle() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\n======= \nConflicting line\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("======= "));
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_end() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\n>>>>>>> branch\nMore content\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains(">>>>>>> "));
        Ok(())
    }

    #[tokio::test]
    async fn test_full_conflict_block() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Before conflict\n<<<<<<< HEAD\nOur changes\n=======\nTheir changes\n>>>>>>> branch\nAfter conflict\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        // Should find all three markers
        assert!(output_str.contains("<<<<<<< "));
        assert!(output_str.contains("======="));
        assert!(output_str.contains(">>>>>>> "));
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_not_at_start() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content <<<<<<< HEAD\n";
        let file_path = create_test_file(&dir, "no_conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        // Should not detect conflict since marker is not at line start
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_crlf() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\r\n=======\r\nConflicting line\r\n";
        let file_path = create_test_file(&dir, "conflict_crlf.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_lf() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\n=======\nConflicting line\n";
        let file_path = create_test_file(&dir, "conflict_lf.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_conflicts() -> Result<()> {
        let dir = tempdir()?;
        let content = b"<<<<<<< HEAD\nFirst\n=======\nSecond\n>>>>>>> branch\nMiddle\n<<<<<<< HEAD\nThird\n=======\nFourth\n>>>>>>> other\n";
        let file_path = create_test_file(&dir, "multiple.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        // Should find all markers from both conflicts (one per line with marker)
        let marker_count = output_str.matches("Merge conflict string").count();
        assert_eq!(marker_count, 6); // 3 markers per conflict * 2 conflicts
        Ok(())
    }

    #[tokio::test]
    async fn test_binary_file_with_conflict() -> Result<()> {
        let dir = tempdir()?;
        let mut content = vec![0xFF, 0xFE, 0xFD];
        content.extend_from_slice(b"\n<<<<<<< HEAD\n");
        let file_path = create_test_file(&dir, "binary.bin", &content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }
}
