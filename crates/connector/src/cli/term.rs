//! What the person sees and answers in the terminal. Wiki: docs/wiki/server-setup.md

use std::io::{BufRead as _, IsTerminal as _, Write as _};

use anyhow::{Context, Result};

/// The terminal of the setup: lines out, choices in — and no question asked where nobody can answer.
pub struct Term {
    out: std::io::Stdout,
    interactive: bool,
}

impl Term {
    pub fn new() -> Self {
        Self {
            out: std::io::stdout(),
            interactive: std::io::stdin().is_terminal(),
        }
    }

    pub fn line(&self, text: &str) -> Result<()> {
        writeln!(self.out.lock(), "{text}").context("cannot write to the terminal")
    }

    pub fn blank(&self) -> Result<()> {
        self.line("")
    }

    pub fn done(&self, text: &str) -> Result<()> {
        self.line(&format!("✓ {text}"))
    }

    pub fn note(&self, text: &str) -> Result<()> {
        self.line(&format!("! {text}"))
    }

    pub fn progress(&self, text: &str) -> Result<()> {
        let mut out = self.out.lock();
        write!(out, "… {text}").context("cannot write to the terminal")?;
        out.flush().context("cannot write to the terminal")
    }

    pub fn tick(&self) -> Result<()> {
        let mut out = self.out.lock();
        write!(out, ".").context("cannot write to the terminal")?;
        out.flush().context("cannot write to the terminal")
    }

    fn ask(&self, prompt: &str) -> Result<String> {
        {
            let mut out = self.out.lock();
            write!(out, "{prompt}").context("cannot write to the terminal")?;
            out.flush().context("cannot write to the terminal")?;
        }
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .context("cannot read the answer")?;
        Ok(answer.trim().to_string())
    }

    /// Pick one of `options` by number; without a terminal, refuse and name the flag that answers instead.
    pub fn choose(&self, options: &[String], flag: &str) -> Result<usize> {
        if options.len() == 1 {
            self.line(&format!("  1. {}", options[0]))?;
            return Ok(0);
        }
        anyhow::ensure!(
            self.interactive,
            "More than one choice and no terminal to ask: pass {flag}"
        );
        for (n, option) in options.iter().enumerate() {
            self.line(&format!("  {}. {option}", n + 1))?;
        }
        loop {
            let answer = self.ask(&format!("Choose 1-{}: ", options.len()))?;
            if let Some(n) = answer
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=options.len()).contains(n))
            {
                return Ok(n - 1);
            }
        }
    }

    /// A yes/no question; `--yes` answers it, and without a terminal it must.
    pub fn confirm(&self, question: &str, yes: bool) -> Result<bool> {
        if yes {
            self.done(&format!("{question} yes (--yes)"))?;
            return Ok(true);
        }
        anyhow::ensure!(
            self.interactive,
            "{question} — no terminal to ask: pass --yes"
        );
        let answer = self.ask(&format!("{question} [y/N] "))?;
        Ok(matches!(answer.as_str(), "y" | "Y" | "yes" | "Yes"))
    }
}
