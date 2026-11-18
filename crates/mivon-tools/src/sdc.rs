//! ENT-31: SDC Timing Constraints — basic format parser.
//!
//! Parses subset of Synopsys Design Constraints (SDC) format:
//! - create_clock
//! - set_input_delay / set_output_delay
//! - set_max_delay / set_min_delay
//! - set_false_path
//! - set_multicycle_path
//! - group_path
//!
//! SDC files define timing constraints for synthesis and STA tools.

use serde::{Deserialize, Serialize};

/// Parsed SDC constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SdcConstraint {
    CreateClock {
        name: String,
        period: f64,
        waveform: Option<(f64, f64)>,
        ports: Vec<String>,
    },
    SetInputDelay {
        delay: f64,
        clock: String,
        ports: Vec<String>,
        max: bool,
    },
    SetOutputDelay {
        delay: f64,
        clock: String,
        ports: Vec<String>,
        max: bool,
    },
    SetMaxDelay {
        delay: f64,
        from: Vec<String>,
        to: Vec<String>,
    },
    SetMinDelay {
        delay: f64,
        from: Vec<String>,
        to: Vec<String>,
    },
    SetFalsePath {
        from: Vec<String>,
        to: Vec<String>,
    },
    SetMulticyclePath {
        setup: u32,
        from: Vec<String>,
        to: Vec<String>,
    },
    GroupPath {
        name: String,
        from: Vec<String>,
        to: Vec<String>,
    },
    Unknown(String),
}

/// Parsed SDC file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdcDocument {
    pub constraints: Vec<SdcConstraint>,
}

impl SdcDocument {
    /// Parse SDC file content.
    pub fn parse(content: &str) -> Result<Self, String> {
        let mut constraints = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(constraint) = parse_line(trimmed) {
                constraints.push(constraint);
            }
        }
        Ok(SdcDocument { constraints })
    }

    /// Load from file.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        Self::parse(&content)
    }

    /// Summary.
    pub fn summary(&self) -> String {
        format!("SDC document: {} constraints", self.constraints.len())
    }

    /// Get all clocks.
    pub fn clocks(&self) -> Vec<&SdcConstraint> {
        self.constraints
            .iter()
            .filter(|c| matches!(c, SdcConstraint::CreateClock { .. }))
            .collect()
    }
}

fn parse_line(line: &str) -> Option<SdcConstraint> {
    let tokens = tokenize(line);
    if tokens.is_empty() {
        return None;
    }

    match tokens[0].as_str() {
        "create_clock" => parse_create_clock(&tokens),
        "set_input_delay" => parse_set_input_delay(&tokens),
        "set_output_delay" => parse_set_output_delay(&tokens),
        "set_max_delay" => parse_set_max_delay(&tokens),
        "set_min_delay" => parse_set_min_delay(&tokens),
        "set_false_path" => parse_set_false_path(&tokens),
        "set_multicycle_path" => parse_set_multicycle_path(&tokens),
        "group_path" => parse_group_path(&tokens),
        _ => Some(SdcConstraint::Unknown(line.to_string())),
    }
}

fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_braces = false;

    for ch in line.chars() {
        match ch {
            '{' => in_braces = true,
            '}' => {
                in_braces = false;
                if !current.trim().is_empty() {
                    tokens.push(current.trim().to_string());
                }
                current.clear();
            }
            ' ' | '\t' if !in_braces => {
                if !current.trim().is_empty() {
                    tokens.push(current.trim().to_string());
                    current.clear();
                }
            }
            '\\' => { /* skip next char */ }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }
    tokens
}

fn parse_create_clock(tokens: &[String]) -> Option<SdcConstraint> {
    let mut name = String::new();
    let mut period = 0.0;
    let mut waveform = None;
    let mut ports = Vec::new();

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "-name" => {
                i += 1;
                name = tokens.get(i).cloned().unwrap_or_default();
            }
            "-period" => {
                i += 1;
                period = tokens.get(i).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            }
            "-waveform" => {
                i += 1;
                let rise: f64 = tokens.get(i).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                i += 1;
                let fall: f64 = tokens.get(i).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                waveform = Some((rise, fall));
            }
            _ => {
                if !tokens[i].starts_with('-') {
                    ports.push(tokens[i].clone());
                }
            }
        }
        i += 1;
    }

    Some(SdcConstraint::CreateClock {
        name,
        period,
        waveform,
        ports,
    })
}

fn parse_set_delay(tokens: &[String], is_input: bool) -> Option<SdcConstraint> {
    let mut delay = 0.0;
    let mut clock = String::new();
    let mut ports = Vec::new();
    let mut max = false;

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "-clock" => {
                i += 1;
                clock = tokens.get(i).cloned().unwrap_or_default();
            }
            "-max" => max = true,
            "-min" => max = false,
            _ => {
                if !tokens[i].starts_with('-') {
                    if delay == 0.0 {
                        delay = tokens[i].parse().ok().unwrap_or(0.0);
                    } else {
                        ports.push(tokens[i].clone());
                    }
                }
            }
        }
        i += 1;
    }

    if is_input {
        Some(SdcConstraint::SetInputDelay {
            delay,
            clock,
            ports,
            max,
        })
    } else {
        Some(SdcConstraint::SetOutputDelay {
            delay,
            clock,
            ports,
            max,
        })
    }
}

fn parse_set_input_delay(tokens: &[String]) -> Option<SdcConstraint> {
    parse_set_delay(tokens, true)
}

fn parse_set_output_delay(tokens: &[String]) -> Option<SdcConstraint> {
    parse_set_delay(tokens, false)
}

fn parse_set_max_delay(tokens: &[String]) -> Option<SdcConstraint> {
    let mut delay = 0.0;
    let mut from = Vec::new();
    let mut to = Vec::new();
    let mut in_from = false;
    let mut in_to = false;

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "-from" => in_from = true,
            "-to" => {
                in_from = false;
                in_to = true;
            }
            _ => {
                if in_from {
                    from.push(tokens[i].clone());
                } else if in_to {
                    to.push(tokens[i].clone());
                } else if delay == 0.0 {
                    delay = tokens[i].parse().ok().unwrap_or(0.0);
                }
            }
        }
        i += 1;
    }

    Some(SdcConstraint::SetMaxDelay { delay, from, to })
}

fn parse_set_min_delay(tokens: &[String]) -> Option<SdcConstraint> {
    let mut delay = 0.0;
    let mut from = Vec::new();
    let mut to = Vec::new();
    let mut in_from = false;
    let mut in_to = false;

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "-from" => in_from = true,
            "-to" => {
                in_from = false;
                in_to = true;
            }
            _ => {
                if in_from {
                    from.push(tokens[i].clone());
                } else if in_to {
                    to.push(tokens[i].clone());
                } else if delay == 0.0 {
                    delay = tokens[i].parse().ok().unwrap_or(0.0);
                }
            }
        }
        i += 1;
    }

    Some(SdcConstraint::SetMinDelay { delay, from, to })
}

fn parse_set_false_path(tokens: &[String]) -> Option<SdcConstraint> {
    let mut from = Vec::new();
    let mut to = Vec::new();
    let mut in_from = false;
    let mut in_to = false;

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "-from" => in_from = true,
            "-to" => {
                in_from = false;
                in_to = true;
            }
            _ => {
                if in_from && !tokens[i].starts_with('-') {
                    from.push(tokens[i].clone());
                } else if in_to && !tokens[i].starts_with('-') {
                    to.push(tokens[i].clone());
                }
            }
        }
        i += 1;
    }

    Some(SdcConstraint::SetFalsePath { from, to })
}

fn parse_set_multicycle_path(tokens: &[String]) -> Option<SdcConstraint> {
    let mut setup = 1u32;
    let mut from = Vec::new();
    let mut to = Vec::new();
    let mut in_from = false;
    let mut in_to = false;

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "-setup" => { /* default */ }
            "-from" => in_from = true,
            "-to" => {
                in_from = false;
                in_to = true;
            }
            _ => {
                if in_from && !tokens[i].starts_with('-') {
                    from.push(tokens[i].clone());
                } else if in_to && !tokens[i].starts_with('-') {
                    to.push(tokens[i].clone());
                } else if setup == 1 {
                    setup = tokens[i].parse().ok().unwrap_or(1);
                }
            }
        }
        i += 1;
    }

    Some(SdcConstraint::SetMulticyclePath { setup, from, to })
}

fn parse_group_path(tokens: &[String]) -> Option<SdcConstraint> {
    let mut name = String::new();
    let mut from = Vec::new();
    let mut to = Vec::new();
    let mut in_from = false;
    let mut in_to = false;

    let mut i = 1;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "-name" => {
                i += 1;
                name = tokens.get(i).cloned().unwrap_or_default();
            }
            "-from" => in_from = true,
            "-to" => {
                in_from = false;
                in_to = true;
            }
            _ => {
                if in_from && !tokens[i].starts_with('-') {
                    from.push(tokens[i].clone());
                } else if in_to && !tokens[i].starts_with('-') {
                    to.push(tokens[i].clone());
                }
            }
        }
        i += 1;
    }

    Some(SdcConstraint::GroupPath { name, from, to })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_create_clock() {
        let sdc = r#"
# Clock definition
create_clock -name clk -period 10.0 -waveform {0 5} [get_ports clk]
"#;
        let doc = SdcDocument::parse(sdc).unwrap();
        assert_eq!(doc.constraints.len(), 1);
        if let SdcConstraint::CreateClock {
            name,
            period,
            ports,
            ..
        } = &doc.constraints[0]
        {
            assert_eq!(name, "clk");
            assert_eq!(*period, 10.0);
            assert!(!ports.is_empty());
        } else {
            panic!("expected CreateClock");
        }
    }

    #[test]
    fn test_parse_input_delay() {
        let sdc = "set_input_delay 2.5 -clock clk [get_ports data_in]";
        let doc = SdcDocument::parse(sdc).unwrap();
        assert_eq!(doc.constraints.len(), 1);
        if let SdcConstraint::SetInputDelay {
            delay,
            clock,
            ports,
            ..
        } = &doc.constraints[0]
        {
            assert_eq!(*delay, 2.5);
            assert_eq!(clock, "clk");
            assert!(!ports.is_empty());
        }
    }

    #[test]
    fn test_parse_false_path() {
        let sdc = "set_false_path -from [get_clocks clk_a] -to [get_clocks clk_b]";
        let doc = SdcDocument::parse(sdc).unwrap();
        assert!(matches!(
            &doc.constraints[0],
            SdcConstraint::SetFalsePath { .. }
        ));
    }

    #[test]
    fn test_parse_multicycle() {
        let sdc = "set_multicycle_path 2 -from [get_ports a] -to [get_ports b]";
        let doc = SdcDocument::parse(sdc).unwrap();
        if let SdcConstraint::SetMulticyclePath { setup, .. } = &doc.constraints[0] {
            assert_eq!(*setup, 2);
        }
    }

    #[test]
    fn test_summary() {
        let sdc = "create_clock -name clk -period 10 [get_ports clk]\nset_false_path -from [get_ports a] -to [get_ports b]";
        let doc = SdcDocument::parse(sdc).unwrap();
        let s = doc.summary();
        assert!(s.contains("2 constraints"));
    }

    #[test]
    fn test_comments_and_empty() {
        let sdc =
            "# comment\n\n# another comment\ncreate_clock -name clk -period 5 [get_ports c]\n";
        let doc = SdcDocument::parse(sdc).unwrap();
        assert_eq!(doc.constraints.len(), 1);
    }

    #[test]
    fn test_sdc_clocks_filter_only_create_clock() {
        let sdc = "create_clock -name clk -period 10 [get_ports clk]\nset_false_path -from [get_ports a] -to [get_ports b]\ncreate_clock -name clk2 -period 20 [get_ports clk2]";
        let doc = SdcDocument::parse(sdc).unwrap();
        let clocks = doc.clocks();
        assert_eq!(clocks.len(), 2);
        assert!(clocks.iter().all(|c| matches!(c, SdcConstraint::CreateClock { .. })));
    }

    #[test]
    fn test_sdc_load_nonexistent_file() {
        let doc = SdcDocument::load(std::path::Path::new("/tmp/opencode/nonexistent.sdc"));
        assert!(doc.is_err());
        assert!(doc.unwrap_err().contains("cannot read"));
    }

    #[test]
    fn test_sdc_summary_multiple_types() {
        let sdc = "create_clock -name clk -period 10 [get_ports clk]\nset_input_delay 2.5 -clock clk [get_ports data]\nset_false_path -from [get_ports a] -to [get_ports b]";
        let doc = SdcDocument::parse(sdc).unwrap();
        let s = doc.summary();
        assert!(s.contains("3 constraints"));
    }
}
