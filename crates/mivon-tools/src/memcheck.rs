//! LINUX-10: Valgrind/Heaptrack Integration — memory profiling helper.
//!
//! Menyediakan API untuk menjalankan valgrind/heaptrack pada binary Mivon
//! dan mem-parsing hasilnya untuk deteksi memory leak.

use std::process::Command;

use serde::{Deserialize, Serialize};

/// Hasil valgrind run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemcheckResult {
    pub tool: String,
    pub exit_code: i32,
    pub definitely_lost: u64,
    pub indirectly_lost: u64,
    pub possibly_lost: u64,
    pub still_reachable: u64,
    pub errors: u64,
    pub summary: String,
}

impl MemcheckResult {
    /// Apakah ada leak signifikan.
    pub fn has_leaks(&self) -> bool {
        self.definitely_lost > 0 || self.possibly_lost > 0
    }

    /// Apakah ada error.
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
}

/// Run valgrind memcheck pada binary.
pub fn run_valgrind(
    binary: &str,
    args: &[String],
    tool_args: &[String],
) -> Result<MemcheckResult, String> {
    let mut cmd = Command::new("valgrind");
    cmd.arg("--leak-check=full")
        .arg("--show-leak-kinds=all")
        .arg("--track-origins=yes")
        .arg("--xml=yes")
        .arg("--xml-file=/dev/stderr")
        .arg(binary);
    cmd.args(args);
    cmd.args(tool_args);

    let output = cmd
        .output()
        .map_err(|e| format!("gagal jalankan valgrind: {}", e))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);

    parse_valgrind_xml(&stderr, exit_code)
}

/// Parse valgrind XML output.
fn parse_valgrind_xml(xml: &str, exit_code: i32) -> Result<MemcheckResult, String> {
    let mut definitely_lost = 0u64;
    let mut indirectly_lost = 0u64;
    let mut possibly_lost = 0u64;
    let mut still_reachable = 0u64;
    let mut errors = 0u64;

    // Simple XML parsing — find <error> counts and <logfile>
    for line in xml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<definitely-lost>") {
            if let Some(val) = extract_tag(trimmed, "definitely-lost") {
                definitely_lost = parse_bytes(val);
            }
        } else if trimmed.starts_with("<indirectly-lost>") {
            if let Some(val) = extract_tag(trimmed, "indirectly-lost") {
                indirectly_lost = parse_bytes(val);
            }
        } else if trimmed.starts_with("<possibly-lost>") {
            if let Some(val) = extract_tag(trimmed, "possibly-lost") {
                possibly_lost = parse_bytes(val);
            }
        } else if trimmed.starts_with("<still-reachable>") {
            if let Some(val) = extract_tag(trimmed, "still-reachable") {
                still_reachable = parse_bytes(val);
            }
        } else if trimmed == "<error>" {
            errors += 1;
        }
    }

    // Fallback: parse text output
    if definitely_lost == 0 && indirectly_lost == 0 && possibly_lost == 0 {
        for line in xml.lines() {
            if line.contains("definitely lost:") {
                if let Some(val) = extract_text_value(line, "definitely lost:") {
                    definitely_lost = val;
                }
            } else if line.contains("indirectly lost:") {
                if let Some(val) = extract_text_value(line, "indirectly lost:") {
                    indirectly_lost = val;
                }
            } else if line.contains("possibly lost:") {
                if let Some(val) = extract_text_value(line, "possibly lost:") {
                    possibly_lost = val;
                }
            } else if line.contains("still reachable:") {
                if let Some(val) = extract_text_value(line, "still reachable:") {
                    still_reachable = val;
                }
            } else if line.contains("Invalid") && line.contains("of") {
                errors += 1;
            }
        }
    }

    Ok(MemcheckResult {
        tool: "valgrind".into(),
        exit_code,
        definitely_lost,
        indirectly_lost,
        possibly_lost,
        still_reachable,
        errors,
        summary: format!(
            "lost: {}B definitely, {}B possibly, {} errors",
            definitely_lost, possibly_lost, errors
        ),
    })
}

fn extract_tag<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let start = format!("<{}>", tag);
    let end = format!("</{}>", tag);
    if let (Some(s), Some(e)) = (line.find(&start), line.find(&end)) {
        Some(&line[s + start.len()..e])
    } else {
        None
    }
}

fn extract_text_value(line: &str, prefix: &str) -> Option<u64> {
    if let Some(pos) = line.find(prefix) {
        let rest = &line[pos + prefix.len()..].trim_start();
        // Parse "512 (256 bytes in 4 blocks)" — extract the byte count
        // Or "0 bytes" or "100 bytes"
        if let Some(paren_pos) = rest.find('(') {
            // Inside parentheses: "256 bytes in 4 blocks"
            let inner = rest[paren_pos + 1..].trim_start();
            let num_str: String = inner.chars().take_while(|c| c.is_ascii_digit()).collect();
            num_str.parse().ok()
        } else {
            // Direct number: "100 bytes"
            let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            num_str.parse().ok()
        }
    } else {
        None
    }
}

fn parse_bytes(s: &str) -> u64 {
    let trimmed = s.trim();
    if let Some(pos) = trimmed.find('(') {
        let inner = &trimmed[pos + 1..];
        if let Some(end) = inner.find(' ') {
            inner[..end].parse().unwrap_or(0)
        } else {
            trimmed.parse().unwrap_or(0)
        }
    } else {
        trimmed.parse().unwrap_or(0)
    }
}

/// Run heaptrack pada binary.
pub fn run_heaptrack(binary: &str, args: &[String]) -> Result<MemcheckResult, String> {
    let mut cmd = Command::new("heaptrack");
    cmd.arg(binary);
    cmd.args(args);

    let output = cmd
        .output()
        .map_err(|e| format!("gagal jalankan heaptrack: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);

    // Parse heaptrack output
    let mut total_allocs = 0u64;
    let mut peak = 0u64;
    let mut leaked = 0u64;

    for line in stdout.lines().chain(stderr.lines()) {
        if line.contains("total allocations:") {
            if let Some(val) = extract_text_value(line, "total allocations:") {
                total_allocs = val;
            }
        } else if line.contains("peak:") {
            if let Some(val) = extract_text_value(line, "peak:") {
                peak = val;
            }
        } else if line.contains("leaked:") {
            if let Some(val) = extract_text_value(line, "leaked:") {
                leaked = val;
            }
        }
    }

    Ok(MemcheckResult {
        tool: "heaptrack".into(),
        exit_code,
        definitely_lost: leaked,
        indirectly_lost: 0,
        possibly_lost: 0,
        still_reachable: 0,
        errors: 0,
        summary: format!(
            "heaptrack: {} allocs, peak {}B, leaked {}B",
            total_allocs, peak, leaked
        ),
    })
}

/// Check apakah valgrind tersedia di system.
pub fn valgrind_available() -> bool {
    Command::new("valgrind")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check apakah heaptrack tersedia di system.
pub fn heaptrack_available() -> bool {
    Command::new("heaptrack")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valgrind_xml() {
        let xml = r#"==1234== HEAP SUMMARY:
==1234==     in use at exit: 1,024 bytes in 10 blocks
==1234==   total heap usage: 1,000 allocs, 990 frees, 100,000 bytes allocated
==1234==
==1234== 100 bytes in 1 blocks definitely lost in loss record 1 of 2
==1234== 200 bytes in 2 blocks possibly lost in loss record 2 of 2
==1234==
==1234== LEAK SUMMARY:
==1234==    definitely lost: 100 bytes in 1 blocks
==1234==    indirectly lost: 0 bytes in 0 blocks
==1234==      possibly lost: 200 bytes in 2 blocks
==1234==    still reachable: 50 bytes in 1 blocks
==1234==         suppressed: 0 bytes in 0 blocks
"#;
        let result = parse_valgrind_xml(xml, 0).unwrap();
        assert_eq!(result.definitely_lost, 100);
        assert_eq!(result.possibly_lost, 200);
        assert!(result.has_leaks());
    }

    #[test]
    fn test_memcheck_no_leaks() {
        let result = MemcheckResult {
            tool: "valgrind".into(),
            exit_code: 0,
            definitely_lost: 0,
            indirectly_lost: 0,
            possibly_lost: 0,
            still_reachable: 0,
            errors: 0,
            summary: "clean".into(),
        };
        assert!(!result.has_leaks());
        assert!(!result.has_errors());
    }

    #[test]
    fn test_parse_text_output() {
        let xml = "==1234== definitely lost: 512 (256 bytes in 4 blocks)\n==1234== possibly lost: 0\n==1234== Invalid read of size 4\n==1234== Invalid write of size 8\n";
        let result = parse_valgrind_xml(xml, 1).unwrap();
        assert_eq!(result.definitely_lost, 256);
        assert_eq!(result.errors, 2);
    }

    #[test]
    fn test_extract_tag() {
        assert_eq!(
            extract_tag("<definitely-lost>100</definitely-lost>", "definitely-lost"),
            Some("100")
        );
        assert_eq!(
            extract_tag("<indirectly-lost>200</indirectly-lost>", "indirectly-lost"),
            Some("200")
        );
        assert_eq!(extract_tag("<tags>foo</tags>", "bar"), None);
        assert_eq!(extract_tag("<foo>bar</foo>", "foo"), Some("bar"));
    }
}
