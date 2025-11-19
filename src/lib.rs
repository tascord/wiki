pub mod helpers;
pub mod wiki;

pub use wiki::{Information, Wiki};

use std::cell::RefCell;
use std::path::PathBuf;

thread_local! {
    static CURRENT_WIKI: RefCell<Option<Wiki>> = RefCell::new(None);
    static USE_GLOBAL: RefCell<bool> = RefCell::new(false);
}

/// Set whether to use the global wiki directory
pub fn set_use_global(use_global: bool) {
    USE_GLOBAL.with(|g| {
        *g.borrow_mut() = use_global;
    });
}

/// Check if we should use global directory
pub fn is_using_global() -> bool {
    USE_GLOBAL.with(|g| *g.borrow())
}

/// Switch to a different wiki context (creates if it doesn't exist)
pub fn switch(wiki_name: String) -> Result<(), String> {
    let use_global = is_using_global();
    let wiki = Wiki::load_or_create(wiki_name, use_global);
    CURRENT_WIKI.with(|w| {
        *w.borrow_mut() = Some(wiki);
    });
    Ok(())
}

/// Commit a fact to the current wiki
pub fn commit(fact: String, tags: Vec<String>) -> Result<uuid::Uuid, String> {
    CURRENT_WIKI.with(|w| {
        let mut wiki_ref = w.borrow_mut();
        if let Some(wiki) = wiki_ref.as_mut() {
            wiki.commit(fact, tags).map_err(|e| e.to_string())
        } else {
            Err("No wiki context selected. Use switch() first.".to_string())
        }
    })
}

/// Recall facts related to a query
pub fn recall(query: &str, tag_filter: Option<&str>) -> Result<Vec<Information>, String> {
    CURRENT_WIKI.with(|w| {
        let wiki_ref = w.borrow();
        if let Some(wiki) = wiki_ref.as_ref() {
            Ok(wiki.recall(query, tag_filter))
        } else {
            Err("No wiki context selected. Use switch() first.".to_string())
        }
    })
}

/// Recall all facts with a specific tag
pub fn recall_by_tag(tag: &str) -> Result<Vec<Information>, String> {
    CURRENT_WIKI.with(|w| {
        let wiki_ref = w.borrow();
        if let Some(wiki) = wiki_ref.as_ref() {
            Ok(wiki.recall_by_tag(tag))
        } else {
            Err("No wiki context selected. Use switch() first.".to_string())
        }
    })
}

/// Build static site generator using mdbook
pub fn book() -> Result<PathBuf, String> {
    CURRENT_WIKI.with(|w| {
        let wiki_ref = w.borrow();
        if let Some(wiki) = wiki_ref.as_ref() {
            wiki.generate_book().map_err(|e| e.to_string())
        } else {
            Err("No wiki context selected. Use switch() first".to_string())
        }
    })
}
