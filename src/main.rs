use clap::{Parser, Subcommand};
use colored::*;
use std::env;
use twk::{commit, recall, recall_by_tag, switch, book, set_use_global};

mod tui;

#[derive(Parser)]
#[command(name = "wk")]
#[command(about = "Home grown knowledge management tool", long_about = None)]
struct Cli {
    /// Use global wiki directory instead of local .wiki/ folder
    #[arg(short = 'g', long = "global", global = true)]
    global: bool,
    
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Commit a fact to memory
    #[command(name = "c", alias = "commit")]
    Commit {
        /// The fact to commit
        fact: String,
        /// Optional tag for the fact
        tag: Option<String>,
    },
    
    /// Recall facts related to a query
    #[command(name = "r", alias = "recall")]
    Recall {
        /// Query to search for, or tag if used alone
        query: Option<String>,
        /// Show fact IDs in the output
        #[arg(long = "id")]
        show_id: bool,
    },
    
    /// Build static site generator
    #[command(name = "book")]
    Book,
    
    /// Switch wiki context (creates if not exists)
    #[command(name = "switch")]
    Switch {
        /// Name of the wiki to switch to
        wikiname: String,
        /// Initialize a local .wiki/ folder in the current directory
        #[arg(short = 'l', long = "local")]
        local: bool,
    },

    /// Launch the TUI
    #[command(name = "tui")]
    Tui,
}

fn main() {
    let cli = Cli::parse();

    // Set whether to use global directory
    set_use_global(cli.global);

    // Get or set default wiki context
    let current_wiki = env::var("TWK_WIKI").unwrap_or_else(|_| "default".to_string());
    
    // Initialize wiki context if no switch command
    if !matches!(cli.command, Some(Commands::Switch { .. })) {
        if let Err(e) = switch(current_wiki.clone()) {
            eprintln!("{} {}", "Error:".red().bold(), e);
            std::process::exit(1);
        }
    }

    match cli.command {
        Some(Commands::Commit { fact, tag }) => {
            let tags = tag.map(|t| vec![t]).unwrap_or_default();
            
            match commit(fact.clone(), tags.clone()) {
                Ok(_) => {
                    if !tags.is_empty() {
                        println!("{} {}", "✓".green().bold(), 
                            tags.iter()
                                .map(|t| format!("[{}]", t.yellow()))
                                .collect::<Vec<_>>()
                                .join(" "));
                    } else {
                        println!("{}", "✓".green().bold());
                    }
                }
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
            }
        }
        
        Some(Commands::Recall { query, show_id }) => {
            match query {
                Some(q) => {
                    // Check if it's a tag query (no spaces, looks like a tag)
                    let results = if q.starts_with('[') && q.ends_with(']') {
                        // Tag query: [tag]
                        let tag = q.trim_matches(|c| c == '[' || c == ']');
                        recall_by_tag(tag)
                    } else {
                        // Regular text query
                        recall(&q, None)
                    };
                    
                    match results {
                        Ok(facts) => {
                            if facts.is_empty() {
                                println!("{}", "No matching facts found.".yellow());
                            } else {
                                for fact in facts.iter() {
                                    // Simple, clean output
                                    print!("{}", fact.data.white());
                                    
                                    if !fact.tags.is_empty() {
                                        print!(" {}", 
                                            fact.tags.iter()
                                                .map(|t| format!("[{}]", t.bright_black()))
                                                .collect::<Vec<_>>()
                                                .join(" "));
                                    }
                                    
                                    if show_id {
                                        print!(" {}", format!("({})", fact.id.to_string().bright_black()));
                                    }
                                    
                                    println!();
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("{} {}", "Error:".red().bold(), e);
                            std::process::exit(1);
                        }
                    }
                }
                None => {
                    // No query provided - could show all or enter TUI mode
                    println!("{}", "Usage: wk r <query> or wk r [tag]".yellow());
                    println!("  {} Search for facts containing query", "wk r \"rust tips\"".bright_black());
                    println!("  {} Recall all facts with tag", "wk r [programming]".bright_black());
                }
            }
        }
        
        Some(Commands::Book) => {
            match book() {
                Ok(output_path) => {
                    println!("{}", "✓ Static site generated".green().bold());
                    println!("  {} {}", "Output:".cyan(), output_path.display().to_string().white());
                    println!();
                    println!("{}", "To view the book:".bright_black());
                    println!("  {}", format!("mdbook serve {}", output_path.parent().unwrap().display()).yellow());
                }
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
            }
        }
        
        Some(Commands::Switch { wikiname, local }) => {
            if local {
                // Create local .wiki/ folder
                if let Err(e) = std::fs::create_dir_all(".wiki") {
                    eprintln!("{} Failed to create .wiki/ folder: {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
                println!("{}", "✓ Created local .wiki/ folder".green().bold());
                println!("  {} {}", "Path:".cyan(), ".wiki/".white());
                println!();
            }
            
            match switch(wikiname.clone()) {
                Ok(_) => {
                    println!("{}", "✓ Switched wiki context".green().bold());
                    println!("  {} {}", "Wiki:".cyan(), wikiname.white());
                    if !local {
                        println!();
                        println!("{}", "To persist this change, set the environment variable:".bright_black());
                        println!("  {}", format!("export TWK_WIKI={}", wikiname).yellow());
                    }
                }
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
            }
        }

        Some(Commands::Tui) => {
            if let Err(e) = tui::run(current_wiki, cli.global) {
                eprintln!("{} {}", "Error:".red().bold(), e);
                std::process::exit(1);
            }
        }
        
        None => {
            // No command - could enter TUI mode in the future
            println!("{}", "TiddlyWiki Knowledge Manager".bright_cyan().bold());
            println!();
            println!("{}", "Usage:".white().bold());
            println!("  {} {}      Commit a fact to memory", "wk c".yellow(), "<fact> [tag]".bright_black());
            println!("  {} {}      Recall facts", "wk r".yellow(), "<query>".bright_black());
            println!("  {} {}         Recall facts by tag", "wk r".yellow(), "[tag]".bright_black());
            println!("  {} {}  Switch wiki context", "wk switch".yellow(), "<name>".bright_black());
            println!("  {} {}         Build static site", "wk book".yellow(), "          ".bright_black());
            println!();
            println!("{} {}", "Current wiki:".cyan(), current_wiki.white());
            println!();
            println!("{}", "Run 'wk --help' for more information".bright_black());
        }
    }
}
