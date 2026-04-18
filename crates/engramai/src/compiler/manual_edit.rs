//! ManualEditManager — handles user edits to topic page sections,
//! marks edited sections for preservation during recompilation (GOAL-comp.7).

use chrono::Utc;

use crate::compiler::types::*;

/// Manages manual edits to topic page sections.
pub struct ManualEditManager;

impl ManualEditManager {
    pub fn new() -> Self {
        Self
    }

    /// Apply a user edit to a specific section of a topic page.
    /// Returns the updated TopicPage with the section marked as user_edited.
    /// If the section doesn't exist, creates a new one.
    pub fn apply_edit(
        &self,
        page: &TopicPage,
        section_heading: &str,
        new_body: &str,
    ) -> Result<TopicPage, KcError> {
        let mut updated = page.clone();
        let now = Utc::now();

        let mut found = false;
        for section in &mut updated.sections {
            if section.heading == section_heading {
                section.body = new_body.to_string();
                section.user_edited = true;
                section.edited_at = Some(now);
                found = true;
                break;
            }
        }

        if !found {
            updated.sections.push(TopicSection {
                heading: section_heading.to_string(),
                body: new_body.to_string(),
                user_edited: true,
                edited_at: Some(now),
            });
        }

        // Update the flat content field to reflect sections
        updated.content = Self::sections_to_content(&updated.sections);
        updated.metadata.updated_at = now;
        updated.version += 1;

        Ok(updated)
    }

    /// Build the "fixed sections" prompt addendum for compilation.
    /// Returns the prompt text to prepend when recompiling a page that has user edits.
    /// Returns None if no user-edited sections exist.
    pub fn build_fixed_sections_prompt(sections: &[TopicSection]) -> Option<String> {
        let edited: Vec<_> = sections.iter().filter(|s| s.user_edited).collect();
        if edited.is_empty() {
            return None;
        }

        let mut lines = vec![
            "FIXED SECTIONS (do not modify these — user has edited them):".to_string(),
        ];
        for section in &edited {
            lines.push(format!("- Section \"{}\": {}", section.heading, section.body));
        }
        lines.push(String::new());
        lines.push(
            "Add new content around fixed sections. If new source material \
             contradicts a fixed section, note the conflict explicitly."
                .to_string(),
        );

        Some(lines.join("\n"))
    }

    /// Parse flat content string into sections.
    /// Sections are delimited by markdown headings (## Heading).
    /// Content before the first heading becomes the "Overview" section.
    pub fn content_to_sections(content: &str) -> Vec<TopicSection> {
        let mut sections = Vec::new();
        let mut current_heading = "Overview".to_string();
        let mut current_body = Vec::new();

        for line in content.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                // Save previous section if it has content
                let body = current_body.join("\n").trim().to_string();
                if !body.is_empty() || current_heading != "Overview" {
                    sections.push(TopicSection {
                        heading: current_heading,
                        body,
                        user_edited: false,
                        edited_at: None,
                    });
                }
                current_heading = heading.trim().to_string();
                current_body = Vec::new();
            } else {
                current_body.push(line.to_string());
            }
        }

        // Don't forget the last section
        let body = current_body.join("\n").trim().to_string();
        if !body.is_empty() || sections.is_empty() {
            sections.push(TopicSection {
                heading: current_heading,
                body,
                user_edited: false,
                edited_at: None,
            });
        }

        sections
    }

    /// Convert sections back to flat content string.
    pub fn sections_to_content(sections: &[TopicSection]) -> String {
        let mut parts = Vec::new();
        for (i, section) in sections.iter().enumerate() {
            if i == 0 && section.heading == "Overview" {
                // Overview section: no heading prefix
                parts.push(section.body.clone());
            } else {
                parts.push(format!("## {}\n\n{}", section.heading, section.body));
            }
        }
        parts.join("\n\n")
    }

    /// Check if a topic page has any user-edited sections.
    pub fn has_user_edits(sections: &[TopicSection]) -> bool {
        sections.iter().any(|s| s.user_edited)
    }

    /// Get list of user-edited section headings (for conflict detection).
    pub fn edited_section_headings(sections: &[TopicSection]) -> Vec<String> {
        sections
            .iter()
            .filter(|s| s.user_edited)
            .map(|s| s.heading.clone())
            .collect()
    }

    /// Reset user_edited flags (e.g., after user confirms recompiled version).
    pub fn clear_edit_flags(sections: &mut [TopicSection]) {
        for section in sections.iter_mut() {
            section.user_edited = false;
            section.edited_at = None;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_page() -> TopicPage {
        let now = Utc::now();
        let content = "Overview text here.\n\n## Background\n\nSome background.\n\n## Details\n\nSome details.";
        TopicPage {
            id: TopicId("test-1".to_owned()),
            title: "Test Topic".to_owned(),
            content: content.to_owned(),
            sections: ManualEditManager::content_to_sections(content),
            summary: "A summary".to_owned(),
            status: TopicStatus::Active,
            version: 1,
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 1,
                source_memory_ids: vec!["mem-1".to_owned()],
                tags: vec![],
                quality_score: Some(0.8),
            },
        }
    }

    #[test]
    fn test_apply_edit_existing_section() {
        let mgr = ManualEditManager::new();
        let page = sample_page();

        let updated = mgr
            .apply_edit(&page, "Background", "Updated background content.")
            .unwrap();

        let bg = updated
            .sections
            .iter()
            .find(|s| s.heading == "Background")
            .unwrap();
        assert_eq!(bg.body, "Updated background content.");
        assert!(bg.user_edited);
        assert!(bg.edited_at.is_some());
    }

    #[test]
    fn test_apply_edit_new_section() {
        let mgr = ManualEditManager::new();
        let page = sample_page();

        let updated = mgr
            .apply_edit(&page, "New Section", "Brand new content.")
            .unwrap();

        let new_sec = updated
            .sections
            .iter()
            .find(|s| s.heading == "New Section")
            .unwrap();
        assert_eq!(new_sec.body, "Brand new content.");
        assert!(new_sec.user_edited);
        assert!(new_sec.edited_at.is_some());
    }

    #[test]
    fn test_build_fixed_sections_prompt_none() {
        let sections = ManualEditManager::content_to_sections("Just some text.");
        let prompt = ManualEditManager::build_fixed_sections_prompt(&sections);
        assert!(prompt.is_none());
    }

    #[test]
    fn test_build_fixed_sections_prompt_with_edits() {
        let mut sections = ManualEditManager::content_to_sections(
            "Overview.\n\n## Details\n\nSome details.",
        );
        sections[1].user_edited = true;

        let prompt = ManualEditManager::build_fixed_sections_prompt(&sections);
        assert!(prompt.is_some());
        let text = prompt.unwrap();
        assert!(text.contains("FIXED SECTIONS"));
        assert!(text.contains("Details"));
    }

    #[test]
    fn test_content_to_sections_roundtrip() {
        let content =
            "Overview text here.\n\n## Background\n\nSome background.\n\n## Details\n\nSome details.";
        let sections = ManualEditManager::content_to_sections(content);
        let reconstructed = ManualEditManager::sections_to_content(&sections);
        assert_eq!(reconstructed, content);
    }

    #[test]
    fn test_content_to_sections_no_headings() {
        let content = "Just a flat paragraph\nwith multiple lines.";
        let sections = ManualEditManager::content_to_sections(content);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading, "Overview");
        assert_eq!(sections[0].body, "Just a flat paragraph\nwith multiple lines.");
    }

    #[test]
    fn test_has_user_edits() {
        let sections = ManualEditManager::content_to_sections("Some text.");
        assert!(!ManualEditManager::has_user_edits(&sections));

        let mgr = ManualEditManager::new();
        let page = sample_page();
        let updated = mgr.apply_edit(&page, "Background", "Edited.").unwrap();
        assert!(ManualEditManager::has_user_edits(&updated.sections));
    }

    #[test]
    fn test_clear_edit_flags() {
        let mgr = ManualEditManager::new();
        let page = sample_page();
        let mut updated = mgr.apply_edit(&page, "Background", "Edited.").unwrap();
        assert!(ManualEditManager::has_user_edits(&updated.sections));

        ManualEditManager::clear_edit_flags(&mut updated.sections);
        assert!(!ManualEditManager::has_user_edits(&updated.sections));
    }

    #[test]
    fn test_apply_edit_bumps_version() {
        let mgr = ManualEditManager::new();
        let page = sample_page();
        assert_eq!(page.version, 1);

        let updated = mgr.apply_edit(&page, "Background", "New.").unwrap();
        assert_eq!(updated.version, 2);
    }

    #[test]
    fn test_edited_section_headings() {
        let mgr = ManualEditManager::new();
        let page = sample_page();

        // No edits
        let headings = ManualEditManager::edited_section_headings(&page.sections);
        assert!(headings.is_empty());

        // Edit one section
        let updated = mgr.apply_edit(&page, "Details", "Changed.").unwrap();
        let headings = ManualEditManager::edited_section_headings(&updated.sections);
        assert_eq!(headings, vec!["Details".to_string()]);
    }
}
