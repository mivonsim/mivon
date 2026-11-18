//! ENT-17: Team Collaboration — shared project config dan code review annotations.
//!
//! Menyediakan fitur kolaborasi untuk tim:
//! - Shared project configuration (.mivon/team.toml)
//! - Code review annotations (inline comments pada kode)
//! - Design review checklists
//! - Review status tracking

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Shared team configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamConfig {
    /// Team name.
    pub team_name: String,
    /// Code style preferences.
    pub style: StyleConfig,
    /// Review rules.
    pub review: ReviewConfig,
    /// Module owners (module_name → owner).
    pub owners: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleConfig {
    pub indent_width: usize,
    pub max_line_length: usize,
    pub naming_convention: String, // "snake_case", "camelCase", "PascalCase"
    pub require_header: bool,
    pub header_template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    pub require_review: bool,
    pub min_reviewers: usize,
    pub require_lint_pass: bool,
    pub require_test_pass: bool,
    pub auto_assign_owners: bool,
}

impl Default for TeamConfig {
    fn default() -> Self {
        TeamConfig {
            team_name: "default".into(),
            style: StyleConfig {
                indent_width: 4,
                max_line_length: 120,
                naming_convention: "snake_case".into(),
                require_header: false,
                header_template: None,
            },
            review: ReviewConfig {
                require_review: true,
                min_reviewers: 1,
                require_lint_pass: true,
                require_test_pass: true,
                auto_assign_owners: true,
            },
            owners: HashMap::new(),
        }
    }
}

impl TeamConfig {
    /// Load dari file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        toml::from_str(&content).map_err(|e| format!("failed to parse: {}", e))
    }

    /// Save ke file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content =
            toml::to_string_pretty(self).map_err(|e| format!("failed to serialize: {}", e))?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, content)
            .map_err(|e| format!("failed to write {}: {}", path.display(), e))
    }

    /// Get owner untuk module.
    pub fn get_owner(&self, module_name: &str) -> Option<&str> {
        self.owners.get(module_name).map(|s| s.as_str())
    }
}

/// Review status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewStatus {
    Open,
    InReview,
    Approved,
    ChangesRequested,
    Merged,
}

impl std::fmt::Display for ReviewStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewStatus::Open => write!(f, "🔵 Open"),
            ReviewStatus::InReview => write!(f, "🟡 In Review"),
            ReviewStatus::Approved => write!(f, "🟢 Approved"),
            ReviewStatus::ChangesRequested => write!(f, "🔴 Changes Requested"),
            ReviewStatus::Merged => write!(f, "✅ Merged"),
        }
    }
}

/// Code review annotation (inline comment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewAnnotation {
    pub id: String,
    pub author: String,
    pub file: String,
    pub line: u32,
    pub col: Option<u32>,
    pub severity: String, // "info", "suggestion", "issue", "question"
    pub message: String,
    pub thread_id: Option<String>, // untuk reply threads
    pub resolved: bool,
    pub created_at: String,
}

/// Design review checklist item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewChecklistItem {
    pub category: String,
    pub item: String,
    pub checked: bool,
    pub assignee: Option<String>,
    pub notes: Option<String>,
}

/// Code review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeReview {
    pub id: String,
    pub title: String,
    pub author: String,
    pub status: ReviewStatus,
    pub annotations: Vec<ReviewAnnotation>,
    pub checklist: Vec<ReviewChecklistItem>,
    pub reviewers: Vec<String>,
    pub files_changed: Vec<String>,
}

impl CodeReview {
    pub fn new(id: &str, title: &str, author: &str) -> Self {
        CodeReview {
            id: id.to_string(),
            title: title.to_string(),
            author: author.to_string(),
            status: ReviewStatus::Open,
            annotations: Vec::new(),
            checklist: default_checklist(),
            reviewers: Vec::new(),
            files_changed: Vec::new(),
        }
    }

    pub fn add_annotation(&mut self, annotation: ReviewAnnotation) {
        self.annotations.push(annotation);
    }

    pub fn add_reviewer(&mut self, reviewer: &str) {
        if !self.reviewers.contains(&reviewer.to_string()) {
            self.reviewers.push(reviewer.to_string());
        }
    }

    pub fn approve(&mut self) {
        self.status = ReviewStatus::Approved;
    }

    pub fn request_changes(&mut self) {
        self.status = ReviewStatus::ChangesRequested;
    }

    /// Progress checklist (persentase yang sudah di-check).
    pub fn checklist_progress(&self) -> f64 {
        if self.checklist.is_empty() {
            return 100.0;
        }
        let checked = self.checklist.iter().filter(|i| i.checked).count();
        checked as f64 / self.checklist.len() as f64 * 100.0
    }

    /// Open annotations count.
    pub fn open_annotations(&self) -> usize {
        self.annotations.iter().filter(|a| !a.resolved).count()
    }

    /// Summary report.
    pub fn report(&self) -> String {
        format!(
            "Review {} — {}\n\
             Status: {}\n\
             Author: {} | Reviewers: {:?}\n\
             Files changed: {}\n\
             Annotations: {} open / {} total\n\
             Checklist: {:.0}% complete\n",
            self.id,
            self.title,
            self.status,
            self.author,
            self.reviewers,
            self.files_changed.len(),
            self.open_annotations(),
            self.annotations.len(),
            self.checklist_progress(),
        )
    }
}

/// Default design review checklist.
fn default_checklist() -> Vec<ReviewChecklistItem> {
    vec![
        ReviewChecklistItem {
            category: "Functionality".into(),
            item: "RTL matches design spec".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Functionality".into(),
            item: "Edge cases covered".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Lint".into(),
            item: "No lint warnings (mlint)".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Lint".into(),
            item: "No unused signals".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Timing".into(),
            item: "No combinational loops".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Timing".into(),
            item: "Critical path identified".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Coverage".into(),
            item: "Test coverage > 80%".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Coverage".into(),
            item: "Assertion coverage included".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Documentation".into(),
            item: "Module documentation present".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Documentation".into(),
            item: "Port descriptions complete".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
        ReviewChecklistItem {
            category: "Review".into(),
            item: "At least 1 reviewer approved".into(),
            checked: false,
            assignee: None,
            notes: None,
        },
    ]
}

/// Review store — kelola semua reviews.
pub struct ReviewStore {
    reviews: Mutex<HashMap<String, CodeReview>>,
}

impl ReviewStore {
    pub fn new() -> Self {
        ReviewStore {
            reviews: Mutex::new(HashMap::new()),
        }
    }

    pub fn create_review(&self, id: &str, title: &str, author: &str) -> CodeReview {
        let review = CodeReview::new(id, title, author);
        if let Ok(mut reviews) = self.reviews.lock() {
            reviews.insert(id.to_string(), review.clone());
        }
        review
    }

    pub fn get_review(&self, id: &str) -> Option<CodeReview> {
        self.reviews.lock().ok()?.get(id).cloned()
    }

    pub fn list_reviews(&self) -> Vec<CodeReview> {
        self.reviews
            .lock()
            .map(|r| r.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn count_by_status(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        if let Ok(reviews) = self.reviews.lock() {
            for review in reviews.values() {
                let key = format!("{}", review.status);
                *counts.entry(key).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Apakah tersedia setidaknya satu ulasan?
    pub fn has_reviews(&self) -> bool {
        self.list_reviews().len() > 0
    }
}

impl Default for ReviewStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_team_config_default() {
        let config = TeamConfig::default();
        assert_eq!(config.style.indent_width, 4);
        assert!(config.review.require_review);
    }

    #[test]
    fn test_review_creation() {
        let store = ReviewStore::new();
        let review = store.create_review("R001", "Fix counter", "alice");
        assert_eq!(review.status, ReviewStatus::Open);
        assert_eq!(review.author, "alice");
    }

    #[test]
    fn test_review_approve() {
        let mut review = CodeReview::new("R001", "Test", "alice");
        review.approve();
        assert_eq!(review.status, ReviewStatus::Approved);
    }

    #[test]
    fn test_checklist_progress() {
        let mut review = CodeReview::new("R001", "Test", "alice");
        let total = review.checklist.len();
        review.checklist[0].checked = true;
        review.checklist[1].checked = true;
        let progress = review.checklist_progress();
        assert!((progress - (2.0 / total as f64 * 100.0)).abs() < 0.1);
    }

    #[test]
    fn test_annotations() {
        let mut review = CodeReview::new("R001", "Test", "alice");
        review.add_annotation(ReviewAnnotation {
            id: "A1".into(),
            author: "bob".into(),
            file: "counter.sv".into(),
            line: 10,
            col: None,
            severity: "issue".into(),
            message: "Unused signal".into(),
            thread_id: None,
            resolved: false,
            created_at: "2026-08-27".into(),
        });
        assert_eq!(review.open_annotations(), 1);
    }

    #[test]
    fn test_store_list() {
        let store = ReviewStore::new();
        store.create_review("R1", "First", "alice");
        store.create_review("R2", "Second", "bob");
        assert_eq!(store.list_reviews().len(), 2);
    }

    #[test]
    fn test_report() {
        let review = CodeReview::new("R1", "Test review", "alice");
        let report = review.report();
        assert!(report.contains("Review R1"));
        assert!(report.contains("alice"));
    }

    #[test]
    fn test_has_reviews() {
        let store = ReviewStore::new();
        assert!(!store.has_reviews(), "kosong awalnya");
        store.create_review("R1", "First", "alice");
        assert!(store.has_reviews(), "setelah ada review");
    }
}
