use nucleo_matcher::{Config, Matcher};
use serde::{Deserialize, Serialize};
use std::fs::create_dir_all;
use std::path::PathBuf;
use uuid::Uuid;

use crate::helpers::Locked;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Information {
    pub id: Uuid,
    pub tags: Vec<String>,
    pub name: String,
    pub data: String,
}

impl Information {
    pub fn path(&self, w: &Wiki) -> PathBuf {
        w.path.join(format!("{}.json", self.id))
    }
}

pub struct Wiki {
    pub name: String,
    pub info: Vec<Locked<Information>>,
    pub path: PathBuf,
}

impl Wiki {
    /// Create a new wiki with the given name
    pub fn new(name: String, use_global: bool) -> Self {
        let path = Self::get_wiki_path(&name, use_global);
        std::fs::create_dir_all(&path).ok();

        Wiki {
            name,
            info: Vec::new(),
            path,
        }
    }

    /// Get the path for a wiki by name
    fn get_wiki_path(name: &str, use_global: bool) -> PathBuf {
        if use_global {
            // Use global user directory
            let mut path = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
            path.push("twk");
            path.push(name);
            path
        } else {
            // Check for local .wiki/ folder first
            let local_path = PathBuf::from(".wiki").join(name);
            if local_path.exists() || PathBuf::from(".wiki").exists() {
                local_path
            } else {
                // Fall back to global if no .wiki/ folder exists
                let mut path = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
                path.push("twk");
                path.push(name);
                path
            }
        }
    }

    /// Load an existing wiki or create a new one
    pub fn load_or_create(name: String, use_global: bool) -> Self {
        let path = Self::get_wiki_path(&name, use_global);

        if path.exists() {
            // Load existing wiki data concurrently
            let info = std::fs::read_dir(&path)
            .ok()
            .map(|entries| {
                let results = Arc::new(Mutex::new(Vec::new()));
                let handles: Vec<_> = entries
                .flatten()
                .filter(|entry| {
                    entry.path().extension().and_then(|s| s.to_str()) == Some("json")
                })
                .map(|entry| {
                    let results = Arc::clone(&results);
                    let json_path = entry.path();
                    thread::spawn(move || {
                    if let Ok(locked) = Locked::<Information>::load(json_path) {
                        results.lock().unwrap().push(locked);
                    }
                    })
                })
                .collect();

                for handle in handles {
                handle.join().ok();
                }

                Arc::try_unwrap(results).unwrap().into_inner().unwrap()
            })
            .unwrap_or_default();

            Wiki { name, info, path }
        } else {
            Self::new(name, use_global)
        }
    }

    /// Commit a fact to the wiki
    pub fn commit(&mut self, fact: String, tags: Vec<String>) -> std::io::Result<Uuid> {
        let id = Uuid::new_v4();
        let info = Information {
            id,
            tags,
            name: fact.clone(),
            data: fact,
        };

        let path = info.path(&self);
        create_dir_all(path.parent().unwrap())?;

        self.info.push(Locked::new(path, info)?);
        Ok(id)
    }

    /// Recall facts related to a query using fuzzy matching
    pub fn recall(&self, query: &str, tag_filter: Option<&str>) -> Vec<Information> {
        use nucleo_matcher::Utf32String;

        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut scored_results: Vec<(u32, Information)> = Vec::new();

        for locked_info in &self.info {
            let info_key = locked_info.read();

            // Filter by tag if specified
            if let Some(tag) = tag_filter {
                if !info_key.tags.contains(&tag.to_string()) {
                    continue;
                }
            }

            // Convert strings to UTF-32 for fuzzy matching
            let haystack_name = Utf32String::from(info_key.name.as_str());
            let haystack_data = Utf32String::from(info_key.data.as_str());
            let needle = Utf32String::from(query);

            // Fuzzy match against name and data
            let name_score = matcher.fuzzy_match(haystack_name.slice(..), needle.slice(..));
            let data_score = matcher.fuzzy_match(haystack_data.slice(..), needle.slice(..));

            // Use the best score
            if let Some(score) = name_score.or(data_score) {
                scored_results.push((
                    score as u32,
                    Information {
                        id: info_key.id,
                        tags: info_key.tags.clone(),
                        name: info_key.name.clone(),
                        data: info_key.data.clone(),
                    },
                ));
            }
        }

        // Sort by score (descending)
        scored_results.sort_by(|a, b| b.0.cmp(&a.0));
        scored_results.into_iter().map(|(_, info)| info).collect()
    }

    /// Get all facts with a specific tag
    pub fn recall_by_tag(&self, tag: &str) -> Vec<Information> {
        let mut results = Vec::new();

        for locked_info in &self.info {
            let info_key = locked_info.read();

            if info_key.tags.contains(&tag.to_string()) {
                results.push(Information {
                    id: info_key.id,
                    tags: info_key.tags.clone(),
                    name: info_key.name.clone(),
                    data: info_key.data.clone(),
                });
            }
        }

        results
    }

    /// Generate mdbook static site
    pub fn generate_book(&self) -> std::io::Result<PathBuf> {
        use std::collections::HashMap;
        use std::io::Write;

        // Create temp directory for mdbook
        let temp_dir = tempfile::tempdir()?;
        let src_dir = temp_dir.path().join("src");
        std::fs::create_dir_all(&src_dir)?;

        // Create book.toml
        let book_toml = temp_dir.path().join("book.toml");
        let mut file = std::fs::File::create(&book_toml)?;
        writeln!(file, "[book]")?;
        writeln!(file, "title = \"{} Wiki\"", self.name)?;
        writeln!(file, "authors = []")?;
        writeln!(file, "language = \"en\"")?;
        writeln!(file, "")?;
        writeln!(file, "[output.html]")?;

        // Collect all facts first
        let mut all_facts: Vec<Information> = Vec::new();
        for locked_info in &self.info {
            let info_key = locked_info.read();
            all_facts.push(Information {
                id: info_key.id,
                tags: info_key.tags.clone(),
                name: info_key.name.clone(),
                data: info_key.data.clone(),
            });
        }

        // Group facts by primary tag (first tag only to avoid duplicates)
        let mut tag_groups: HashMap<String, Vec<&Information>> = HashMap::new();
        let mut untagged: Vec<&Information> = Vec::new();

        for fact in &all_facts {
            if fact.tags.is_empty() {
                untagged.push(fact);
            } else {
                // Use only the first tag for categorization
                let primary_tag = &fact.tags[0];
                tag_groups
                    .entry(primary_tag.clone())
                    .or_insert_with(Vec::new)
                    .push(fact);
            }
        }

        // Create SUMMARY.md
        let summary_path = src_dir.join("SUMMARY.md");
        let mut summary = std::fs::File::create(&summary_path)?;
        writeln!(summary, "# Summary")?;
        writeln!(summary, "")?;
        writeln!(summary, "[Introduction](./intro.md)")?;
        writeln!(summary, "")?;

        // Add sections by tag
        let mut sorted_tags: Vec<_> = tag_groups.keys().collect();
        sorted_tags.sort();

        for tag in sorted_tags {
            writeln!(summary, "# {}\n", tag)?;
            if let Some(facts) = tag_groups.get(tag) {
                for fact in facts {
                    let filename = format!("{}.md", fact.id);
                    writeln!(summary, "- [{}](./{})", fact.name, filename)?;
                }
            }
            writeln!(summary, "")?;
        }

        if !untagged.is_empty() {
            writeln!(summary, "# Untagged\n")?;
            for fact in untagged {
                let filename = format!("{}.md", fact.id);
                writeln!(summary, "- [{}](./{})", fact.name, filename)?;
            }
        }

        // Create intro.md
        let intro_path = src_dir.join("intro.md");
        let mut intro = std::fs::File::create(&intro_path)?;
        writeln!(intro, "# {} Wiki", self.name)?;
        writeln!(intro, "")?;
        writeln!(
            intro,
            "This is an automatically generated wiki containing {} facts.",
            self.info.len()
        )?;

        // Create individual fact pages
        for locked_info in &self.info {
            let info_key = locked_info.read();
            let fact_path = src_dir.join(format!("{}.md", info_key.id));
            let mut fact_file = std::fs::File::create(&fact_path)?;

            writeln!(fact_file, "# {}\n", info_key.name)?;
            writeln!(fact_file, "{}\n", info_key.data)?;

            if !info_key.tags.is_empty() {
                writeln!(fact_file, "---\n")?;
                writeln!(fact_file, "**Tags:** {}\n", info_key.tags.join(", "))?;
            }
        }

        // Build the book with mdbook
        let output_dir = self.path.parent().unwrap_or(&self.path).join("book");

        // Ensure output directory parent exists
        if let Some(parent) = output_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Convert to absolute path
        let abs_output_dir = std::fs::canonicalize(&output_dir).unwrap_or_else(|_| {
            // If canonicalize fails (directory doesn't exist yet), build it manually
            std::env::current_dir()
                .unwrap_or_default()
                .join(&output_dir)
        });

        let status = std::process::Command::new("mdbook")
            .arg("build")
            .arg(temp_dir.path())
            .arg("-d")
            .arg(&abs_output_dir)
            .status()?;

        if !status.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "mdbook build failed",
            ));
        }

        // Keep temp_dir alive until here
        drop(temp_dir);

        Ok(output_dir)
    }
}
