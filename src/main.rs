use anyhow::{bail, Context, Result};
use colored::Colorize;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::process::Command;

// ─── Auth / credential types ──────────────────────────────────────────────────

/// Represents the tokens block inside auth.json
#[derive(Debug, Deserialize)]
struct TokenBlock {
    access_token: Option<String>,
    account_id: Option<String>,
}

/// Top-level auth.json schema used by the Codex CLI
#[derive(Debug, Deserialize)]
struct AuthDotJson {
    /// OAuth flow credentials
    tokens: Option<TokenBlock>,
    /// Fallback: plain API key stored directly
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
}

#[derive(Debug)]
struct Credentials {
    access_token: String,
    account_id: Option<String>,
    /// true = OAuth (can hit /wham/usage); false = API key only
    is_oauth: bool,
}

// ─── API response types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
struct RateWindow {
    /// 0–100 percent used
    used_percent: Option<f64>,
    /// seconds until window resets
    reset_after_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RateLimit {
    primary_window: Option<RateWindow>,   // 5-hour window
    secondary_window: Option<RateWindow>, // 7-day window
    limit_reached: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WhamUsage {
    plan_type: Option<String>,
    rate_limit: Option<RateLimit>,
}

// ─── Credential discovery ─────────────────────────────────────────────────────

/// Try to find a usable token, in priority order:
///   1. OPENAI_API_KEY env var
///   2. CODEX_ACCESS_TOKEN env var  (OAuth override)
///   3. ~/.codex/auth.json  (Codex CLI default location)
///   4. ~/.config/codex/auth.json  (XDG alternative)
///   5. macOS Keychain entry "Codex" (if security tool available)
fn get_credentials() -> Result<Credentials> {
    // 1. Env-var overrides
    if let Ok(token) = std::env::var("CODEX_ACCESS_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            let account_id = std::env::var("CODEX_ACCOUNT_ID").ok();
            return Ok(Credentials {
                access_token: token,
                account_id,
                is_oauth: true,
            });
        }
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(Credentials {
                access_token: key,
                account_id: None,
                is_oauth: false,
            });
        }
    }

    // 2. auth.json file locations
    let home = std::env::var_os("HOME").unwrap_or_default();
    let home = std::path::Path::new(&home);
    let candidates = [
        home.join(".codex").join("auth.json"),
        home.join(".config").join("codex").join("auth.json"),
    ];
    for path in &candidates {
        if path.exists() {
            if let Ok(creds) = read_auth_json(path) {
                return Ok(creds);
            }
        }
    }

    // 3. macOS Keychain (service "Codex")
    if let Ok(creds) = read_keychain() {
        return Ok(creds);
    }

    bail!(
        "No OpenAI / Codex credentials found.\n\
         Tried:\n\
         • CODEX_ACCESS_TOKEN / OPENAI_API_KEY env vars\n\
         • ~/.codex/auth.json\n\
         • ~/.config/codex/auth.json\n\
         • macOS Keychain (service \"Codex\")\n\n\
         Log in with:  codex login\n\
         Or set:       export OPENAI_API_KEY=sk-..."
    )
}

fn read_auth_json(path: &std::path::Path) -> Result<Credentials> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Could not read {}", path.display()))?;
    let auth: AuthDotJson = serde_json::from_str(raw.trim())
        .with_context(|| format!("Could not parse {}", path.display()))?;

    // Prefer OAuth tokens over plain API key
    if let Some(ref tokens) = auth.tokens {
        if let Some(ref access_token) = tokens.access_token {
            if !access_token.is_empty() {
                return Ok(Credentials {
                    access_token: access_token.clone(),
                    account_id: tokens.account_id.clone(),
                    is_oauth: true,
                });
            }
        }
    }
    if let Some(key) = auth.openai_api_key {
        if !key.is_empty() {
            return Ok(Credentials {
                access_token: key,
                account_id: None,
                is_oauth: false,
            });
        }
    }
    bail!("auth.json found but contained no usable token")
}

fn read_keychain() -> Result<Credentials> {
    // Try a few plausible macOS Keychain service names used by Codex CLI
    let service_names = ["Codex", "codex", "openai-codex", "Codex CLI"];
    for service in &service_names {
        let output = Command::new("security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                let raw = String::from_utf8_lossy(&out.stdout);
                let raw = raw.trim();
                if !raw.is_empty() {
                    // The value might be a JSON blob or a bare token
                    if let Ok(auth) = serde_json::from_str::<AuthDotJson>(raw) {
                        if let Ok(creds) = extract_from_auth(auth) {
                            return Ok(creds);
                        }
                    }
                    // Treat as raw access token
                    return Ok(Credentials {
                        access_token: raw.to_string(),
                        account_id: None,
                        is_oauth: true,
                    });
                }
            }
        }
    }
    bail!("Codex credentials not found in macOS Keychain")
}

fn extract_from_auth(auth: AuthDotJson) -> Result<Credentials> {
    if let Some(tokens) = auth.tokens {
        if let Some(access_token) = tokens.access_token {
            if !access_token.is_empty() {
                return Ok(Credentials {
                    access_token,
                    account_id: tokens.account_id,
                    is_oauth: true,
                });
            }
        }
    }
    if let Some(key) = auth.openai_api_key {
        if !key.is_empty() {
            return Ok(Credentials {
                access_token: key,
                account_id: None,
                is_oauth: false,
            });
        }
    }
    bail!("no usable token in auth structure")
}

// ─── API call ─────────────────────────────────────────────────────────────────

fn fetch_usage(creds: &Credentials) -> Result<WhamUsage> {
    if !creds.is_oauth {
        bail!(
            "Only an API key was found — Codex usage limits are only visible \
             via an OAuth session token.\n\
             Log in with:  codex login"
        );
    }

    let client = Client::new();
    let mut req = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .header("Authorization", format!("Bearer {}", creds.access_token))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header(
            "User-Agent",
            "Mozilla/5.0 (compatible; codex-usage/0.1)",
        );

    if let Some(ref account_id) = creds.account_id {
        req = req.header("chatgpt-account-id", account_id);
    }

    let resp = req.send().context("Failed to reach ChatGPT API")?;
    let status = resp.status();

    if status.as_u16() == 401 || status.as_u16() == 403 {
        bail!(
            "Token expired or unauthorised (HTTP {status}).\n\
             Try:  codex logout && codex login"
        );
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        bail!("API returned HTTP {status}: {body}");
    }

    // Parse — be lenient; the schema may evolve
    let text = resp.text().context("Failed to read response body")?;
    serde_json::from_str::<WhamUsage>(&text)
        .with_context(|| format!("Failed to parse usage response: {text}"))
}

// ─── Display helpers ──────────────────────────────────────────────────────────

fn usage_bar(pct: f64, width: usize) -> colored::ColoredString {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
    if pct >= 90.0 {
        bar.red().bold()
    } else if pct >= 70.0 {
        bar.yellow()
    } else {
        bar.green()
    }
}

fn format_reset(reset_secs: Option<u64>) -> String {
    let Some(secs) = reset_secs else {
        return "—".dimmed().to_string();
    };
    if secs == 0 {
        return "now".green().to_string();
    }
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;
    if days > 0 {
        format!("in {}d {}h", days, hours % 24).normal().to_string()
    } else if hours > 0 {
        format!("in {}h {}m", hours, mins % 60)
            .normal()
            .to_string()
    } else {
        format!("in {}m", mins).yellow().to_string()
    }
}

fn pct_coloured(pct: f64) -> colored::ColoredString {
    let s = format!("{:5.1}%", pct);
    if pct >= 90.0 {
        s.red().bold()
    } else if pct >= 70.0 {
        s.yellow()
    } else {
        s.green()
    }
}

fn print_window_fancy(label: &str, window: &Option<RateWindow>, bar_width: usize) {
    match window {
        None => {
            println!("  {:<18} {}", label, "not available".dimmed());
        }
        Some(w) => {
            let pct_used = w.used_percent.unwrap_or(0.0).min(100.0);
            let bar = usage_bar(pct_used, bar_width);
            let pct_str = pct_coloured(pct_used);
            println!(
                "  {:<18} {} {} resets {}",
                label.bold(),
                bar,
                pct_str,
                format_reset(w.reset_after_seconds)
            );
        }
    }
}

fn print_window_plain(label: &str, window: &Option<RateWindow>) {
    match window {
        None => println!("{}: N/A", label),
        Some(w) => {
            let pct = w.used_percent.unwrap_or(0.0).min(100.0);
            let reset = w
                .reset_after_seconds
                .map(|s| format!("{}s", s))
                .unwrap_or_else(|| "—".to_string());
            println!("{}: {:.1}% used  Resets in: {}", label, pct, reset);
        }
    }
}

// ─── Entry points ─────────────────────────────────────────────────────────────

fn main() {
    if let Err(e) = run() {
        eprintln!("\n  {} {}\n", "Error:".red().bold(), e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let plain = std::env::args().any(|a| a == "--plain" || a == "-p");

    if !plain {
        println!();
        print!("  {} Fetching usage data... ", "◆".cyan());
        // flush so the user sees it immediately
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }

    let creds = get_credentials()?;
    let usage = fetch_usage(&creds)?;

    // Extract windows
    let rl = usage.rate_limit.as_ref();
    let primary = rl.and_then(|r| r.primary_window.as_ref());
    let secondary = rl.and_then(|r| r.secondary_window.as_ref());
    let limit_reached = rl.and_then(|r| r.limit_reached).unwrap_or(false);

    // ── Plain output ──────────────────────────────────────────────────────────
    if plain {
        let plan = usage.plan_type.as_deref().unwrap_or("unknown");
        println!("Plan: {}", plan.to_uppercase());
        print_window_plain("5hr window", &rl.and_then(|r| r.primary_window.clone()));
        print_window_plain("7day window", &rl.and_then(|r| r.secondary_window.clone()));
        if limit_reached {
            println!("Status: LIMIT REACHED");
        }
        return Ok(());
    }

    // ── Fancy output ──────────────────────────────────────────────────────────
    // Clear the "fetching" line
    print!("\r{}\r", " ".repeat(55));

    let plan = usage
        .plan_type
        .as_deref()
        .unwrap_or("unknown")
        .to_uppercase();

    println!(
        "  {} OpenAI {} Plan — Codex Usage Limits",
        "◆".cyan().bold(),
        plan.yellow().bold()
    );
    println!("  {}", "─".repeat(67).dimmed());

    let bar_width = 28;
    print_window_fancy(
        "5-hour session",
        &rl.and_then(|r| r.primary_window.clone()),
        bar_width,
    );
    print_window_fancy(
        "7-day rolling",
        &rl.and_then(|r| r.secondary_window.clone()),
        bar_width,
    );

    println!("  {}", "─".repeat(67).dimmed());

    // Summary hint
    let highest = [primary, secondary]
        .iter()
        .filter_map(|w| w.map(|w| w.used_percent.unwrap_or(0.0)))
        .fold(0.0_f64, f64::max);

    if limit_reached || highest >= 100.0 {
        println!(
            "\n  {} Limit reached — check your reset time above.",
            "✗".red().bold()
        );
    } else if highest >= 90.0 {
        println!(
            "\n  {} Nearly at your limit — check reset time above.",
            "⚠".red().bold()
        );
    } else if highest >= 70.0 {
        println!(
            "\n  {} Usage is elevated — consider pacing your session.",
            "△".yellow()
        );
    } else {
        println!(
            "\n  {} Looking good — plenty of capacity remaining.",
            "✓".green()
        );
    }

    println!();
    Ok(())
}
