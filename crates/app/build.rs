//! Generates shell completions and a manpage at build time.
//!
//! Both are also available at runtime through `--completions` and `--man`, so
//! `cargo install` users are not left without them. These build-time copies are
//! what distribution packages pick up out of `OUT_DIR`.

use std::path::PathBuf;
use std::{env, fs};

use clap_complete::Shell;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let Some(out_dir) = env::var_os("OUT_DIR").map(PathBuf::from) else {
        return;
    };

    let mut command = bettershot_cli::command();

    let completions = out_dir.join("completions");
    if let Err(e) = fs::create_dir_all(&completions) {
        println!("cargo:warning=could not create the completions directory: {e}");
        return;
    }
    for shell in [
        Shell::Bash,
        Shell::Zsh,
        Shell::Fish,
        Shell::Elvish,
        Shell::PowerShell,
    ] {
        if let Err(e) =
            clap_complete::generate_to(shell, &mut command, bettershot_cli::BIN_NAME, &completions)
        {
            // A missing completion file is not worth failing a build over.
            println!("cargo:warning=could not generate {shell} completions: {e}");
        }
    }

    let mut page = Vec::new();
    match clap_mangen::Man::new(command).render(&mut page) {
        Ok(()) => {
            let path = out_dir.join(format!("{}.1", bettershot_cli::BIN_NAME));
            if let Err(e) = fs::write(&path, page) {
                println!("cargo:warning=could not write the manpage: {e}");
            }
        }
        Err(e) => println!("cargo:warning=could not render the manpage: {e}"),
    }
}
