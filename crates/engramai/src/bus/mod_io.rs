//! Module Reader/Writer — Parse and update agent workspace files.
//!
//! Handles SOUL.md, HEARTBEAT.md, and IDENTITY.md with structure-preserving updates.

use std::fs;
use std::path::Path;

/// A drive/priority extracted from SOUL.md.
#[derive(Debug, Clone, PartialEq)]
pub struct Drive {
    /// The drive name/key (e.g., "curiosity", "helpfulness")
    pub name: String,
    /// The drive description/value
    pub description: String,
    /// Keywords for alignment matching
    pub keywords: Vec<String>,
}

impl Drive {
    /// Extract keywords from the drive name and description.
    pub fn extract_keywords(&self) -> Vec<String> {
        let mut keywords = Vec::new();
        
        // Add name as keyword
        keywords.push(self.name.to_lowercase());
        
        // Extract significant words from description (3+ chars, not stopwords)
        let stopwords = ["the", "and", "for", "with", "that", "this", "from", "are", "was", "but"];
        for word in self.description.split_whitespace() {
            let clean: String = word.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            if clean.len() >= 3 && !stopwords.contains(&clean.as_str()) {
                keywords.push(clean);
            }
        }
        
        keywords.sort();
        keywords.dedup();
        keywords
    }
}

/// A task from HEARTBEAT.md with completion status.
#[derive(Debug, Clone, PartialEq)]
pub struct HeartbeatTask {
    /// Task description
    pub description: String,
    /// Whether the task is completed (checkbox checked)
    pub completed: bool,
    /// Original line for preservation
    pub original_line: String,
}

/// Identity fields from IDENTITY.md.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub name: Option<String>,
    pub creature: Option<String>,
    pub vibe: Option<String>,
    pub emoji: Option<String>,
}

/// Parse SOUL.md to extract drives/priorities.
///
/// Looks for:
/// - `key: value` pairs
/// - `- item` bullet points (treated as drives with item as both name and description)
/// - Section headers starting with `#` are used as context
pub fn parse_soul(content: &str) -> Vec<Drive> {
    let mut drives = Vec::new();
    let mut current_section = String::new();
    
    for line in content.lines() {
        let trimmed = line.trim();
        
        // Track section headers
        if trimmed.starts_with('#') {
            current_section = trimmed.trim_start_matches('#').trim().to_string();
            continue;
        }
        
        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }
        
        // Parse key: value pairs
        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim();
            let value = trimmed[colon_pos + 1..].trim();
            
            // Skip if key looks like a URL or is a section-like thing
            if !key.contains('/') && !key.is_empty() && !value.is_empty() {
                let mut drive = Drive {
                    name: key.to_string(),
                    description: value.to_string(),
                    keywords: Vec::new(),
                };
                drive.keywords = drive.extract_keywords();
                drives.push(drive);
                continue;
            }
        }
        
        // Parse bullet points
        if trimmed.starts_with('-') || trimmed.starts_with('*') {
            let item = trimmed[1..].trim();
            if !item.is_empty() {
                // Use section as context if available
                let name = if !current_section.is_empty() {
                    format!("{}/{}", current_section, item.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
                } else {
                    item.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
                };
                
                let mut drive = Drive {
                    name,
                    description: item.to_string(),
                    keywords: Vec::new(),
                };
                drive.keywords = drive.extract_keywords();
                drives.push(drive);
            }
        }
    }
    
    drives
}

/// Parse HEARTBEAT.md to extract tasks with completion status.
///
/// Looks for:
/// - `- [ ] task` (uncompleted)
/// - `- [x] task` or `- [X] task` (completed)
pub fn parse_heartbeat(content: &str) -> Vec<HeartbeatTask> {
    let mut tasks = Vec::new();
    
    for line in content.lines() {
        let trimmed = line.trim();
        
        // Parse checkbox items
        if let Some(stripped) = trimmed.strip_prefix("- [") {
            if let Some(bracket_end) = stripped.find(']') {
                let checkbox_content = &stripped[..bracket_end];
                let completed = checkbox_content.eq_ignore_ascii_case("x");
                let description = stripped[bracket_end + 1..].trim().to_string();
                
                if !description.is_empty() {
                    tasks.push(HeartbeatTask {
                        description,
                        completed,
                        original_line: line.to_string(),
                    });
                }
            }
        }
    }
    
    tasks
}

/// Parse IDENTITY.md to extract identity fields.
///
/// Looks for:
/// - `name: value`
/// - `creature: value`
/// - `vibe: value`
/// - `emoji: value`
pub fn parse_identity(content: &str) -> Identity {
    let mut identity = Identity::default();
    
    for line in content.lines() {
        let _trimmed = line.trim().to_lowercase();
        
        if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].trim().to_lowercase();
            let value = line[colon_pos + 1..].trim().to_string();
            
            if value.is_empty() {
                continue;
            }
            
            match key.as_str() {
                "name" => identity.name = Some(value),
                "creature" => identity.creature = Some(value),
                "vibe" => identity.vibe = Some(value),
                "emoji" => identity.emoji = Some(value),
                _ => {}
            }
        }
    }
    
    identity
}

/// Read and parse SOUL.md from workspace directory.
pub fn read_soul<P: AsRef<Path>>(workspace_dir: P) -> Result<Vec<Drive>, std::io::Error> {
    let path = workspace_dir.as_ref().join("SOUL.md");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path)?;
    Ok(parse_soul(&content))
}

/// Read and parse HEARTBEAT.md from workspace directory.
pub fn read_heartbeat<P: AsRef<Path>>(workspace_dir: P) -> Result<Vec<HeartbeatTask>, std::io::Error> {
    let path = workspace_dir.as_ref().join("HEARTBEAT.md");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path)?;
    Ok(parse_heartbeat(&content))
}

/// Read and parse IDENTITY.md from workspace directory.
pub fn read_identity<P: AsRef<Path>>(workspace_dir: P) -> Result<Identity, std::io::Error> {
    let path = workspace_dir.as_ref().join("IDENTITY.md");
    if !path.exists() {
        return Ok(Identity::default());
    }
    let content = fs::read_to_string(path)?;
    Ok(parse_identity(&content))
}

/// Update a specific field in SOUL.md (key: value pair).
/// Preserves document structure, only modifies the specified field.
pub fn update_soul_field<P: AsRef<Path>>(
    workspace_dir: P,
    key: &str,
    new_value: &str,
) -> Result<bool, std::io::Error> {
    let path = workspace_dir.as_ref().join("SOUL.md");
    if !path.exists() {
        return Ok(false);
    }
    
    let content = fs::read_to_string(&path)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut updated = false;
    
    for line in &mut lines {
        if let Some(colon_pos) = line.find(':') {
            let line_key = line[..colon_pos].trim();
            if line_key.eq_ignore_ascii_case(key) {
                *line = format!("{}: {}", line_key, new_value);
                updated = true;
                break;
            }
        }
    }
    
    if updated {
        fs::write(path, lines.join("\n"))?;
    }
    
    Ok(updated)
}

/// Add a new drive to SOUL.md at the end.
pub fn add_soul_drive<P: AsRef<Path>>(
    workspace_dir: P,
    key: &str,
    value: &str,
) -> Result<(), std::io::Error> {
    let path = workspace_dir.as_ref().join("SOUL.md");
    let mut content = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!("{}: {}\n", key, value));
    
    fs::write(path, content)?;
    Ok(())
}

/// Update a task completion status in HEARTBEAT.md.
pub fn update_heartbeat_task<P: AsRef<Path>>(
    workspace_dir: P,
    task_description: &str,
    completed: bool,
) -> Result<bool, std::io::Error> {
    let path = workspace_dir.as_ref().join("HEARTBEAT.md");
    if !path.exists() {
        return Ok(false);
    }
    
    let content = fs::read_to_string(&path)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut updated = false;
    
    let checkbox_mark = if completed { "x" } else { " " };
    
    for line in &mut lines {
        let trimmed = line.trim();
        if let Some(stripped) = trimmed.strip_prefix("- [") {
            if let Some(bracket_end) = stripped.find(']') {
                let desc = stripped[bracket_end + 1..].trim();
                if desc.eq_ignore_ascii_case(task_description) {
                    // Preserve indentation
                    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                    *line = format!("{}- [{}] {}", indent, checkbox_mark, desc);
                    updated = true;
                    break;
                }
            }
        }
    }
    
    if updated {
        fs::write(path, lines.join("\n"))?;
    }
    
    Ok(updated)
}

/// Add a new task to HEARTBEAT.md.
pub fn add_heartbeat_task<P: AsRef<Path>>(
    workspace_dir: P,
    description: &str,
) -> Result<(), std::io::Error> {
    let path = workspace_dir.as_ref().join("HEARTBEAT.md");
    let mut content = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!("- [ ] {}\n", description));
    
    fs::write(path, content)?;
    Ok(())
}

/// Update a field in IDENTITY.md.
pub fn update_identity_field<P: AsRef<Path>>(
    workspace_dir: P,
    field: &str,
    new_value: &str,
) -> Result<bool, std::io::Error> {
    let path = workspace_dir.as_ref().join("IDENTITY.md");
    if !path.exists() {
        return Ok(false);
    }
    
    let content = fs::read_to_string(&path)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut updated = false;
    
    for line in &mut lines {
        if let Some(colon_pos) = line.find(':') {
            let line_key = line[..colon_pos].trim();
            if line_key.eq_ignore_ascii_case(field) {
                *line = format!("{}: {}", line_key, new_value);
                updated = true;
                break;
            }
        }
    }
    
    if updated {
        fs::write(path, lines.join("\n"))?;
    }
    
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[test]
    fn test_parse_soul_key_value() {
        let content = r#"
# Core Drives
curiosity: Always seek to understand more
helpfulness: Assist the user effectively

# Secondary
patience: Wait for the right moment
"#;
        let drives = parse_soul(content);
        assert_eq!(drives.len(), 3);
        assert_eq!(drives[0].name, "curiosity");
        assert!(drives[0].description.contains("understand"));
    }
    
    #[test]
    fn test_parse_soul_bullets() {
        let content = r#"
# Values
- Be honest and direct
- Learn from mistakes
"#;
        let drives = parse_soul(content);
        assert_eq!(drives.len(), 2);
        assert!(drives[0].name.contains("Values"));
    }
    
    #[test]
    fn test_parse_heartbeat() {
        let content = r#"
# Daily Tasks
- [ ] Check emails
- [x] Review calendar
- [ ] Run consolidation
"#;
        let tasks = parse_heartbeat(content);
        assert_eq!(tasks.len(), 3);
        assert!(!tasks[0].completed);
        assert!(tasks[1].completed);
        assert_eq!(tasks[0].description, "Check emails");
    }
    
    #[test]
    fn test_parse_identity() {
        let content = r#"
name: Clawd
creature: Cat
vibe: curious and playful
emoji: 🐱
"#;
        let identity = parse_identity(content);
        assert_eq!(identity.name, Some("Clawd".to_string()));
        assert_eq!(identity.creature, Some("Cat".to_string()));
        assert_eq!(identity.emoji, Some("🐱".to_string()));
    }
    
    #[test]
    fn test_drive_keywords() {
        let drive = Drive {
            name: "curiosity".to_string(),
            description: "Always seek to understand new concepts deeply".to_string(),
            keywords: Vec::new(),
        };
        let keywords = drive.extract_keywords();
        assert!(keywords.contains(&"curiosity".to_string()));
        assert!(keywords.contains(&"understand".to_string()));
        assert!(keywords.contains(&"concepts".to_string()));
    }
    
    #[test]
    fn test_write_operations() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = tmpdir.path();
        
        // Test SOUL.md
        fs::write(workspace.join("SOUL.md"), "curiosity: old value\n").unwrap();
        let updated = update_soul_field(workspace, "curiosity", "new value").unwrap();
        assert!(updated);
        let content = fs::read_to_string(workspace.join("SOUL.md")).unwrap();
        assert!(content.contains("new value"));
        
        // Test HEARTBEAT.md
        fs::write(workspace.join("HEARTBEAT.md"), "- [ ] test task\n").unwrap();
        let updated = update_heartbeat_task(workspace, "test task", true).unwrap();
        assert!(updated);
        let content = fs::read_to_string(workspace.join("HEARTBEAT.md")).unwrap();
        assert!(content.contains("- [x]"));
    }
}
