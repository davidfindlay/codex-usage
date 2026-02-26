# codex-usage

Check your ChatGPT / OpenAI Codex CLI plan usage limits from the terminal — identical in spirit to [`claude-usage`](../claude-usage).

## Usage

```
codex-usage          # nice coloured output
codex-usage --plain  # plain text, great for scripts / watch
codex-usage -p       # same as --plain
```

### Fancy mode

```
  ◆ OpenAI PRO Plan — Codex Usage Limits
  ───────────────────────────────────────────────────────────────────
  5-hour session     ████████░░░░░░░░░░░░░░░░░░░░  28.3% resets in 3h 12m
  7-day rolling      ████░░░░░░░░░░░░░░░░░░░░░░░░  15.1% resets in 4d 6h
  ───────────────────────────────────────────────────────────────────

  ✓ Looking good — plenty of capacity remaining.
```

### Plain mode

```
Plan: PRO
5hr window: 28.3% used  Resets in: 11520s
7day window: 15.1% used  Resets in: 367200s
```

## Credential discovery (in order)

| Priority | Source |
|----------|--------|
| 1 | `CODEX_ACCESS_TOKEN` env var (+ optional `CODEX_ACCOUNT_ID`) |
| 2 | `OPENAI_API_KEY` env var *(API key only — cannot show usage limits)* |
| 3 | `~/.codex/auth.json` — written by `codex login` |
| 4 | `~/.config/codex/auth.json` — XDG alternative |
| 5 | macOS Keychain (service name `Codex`) |

> **Note:** Usage limits are only available via an OAuth session token (from  
> `codex login`). A plain API key will authenticate but cannot retrieve limit  
> data from the `/wham/usage` endpoint.

## Install

```bash
# from source
cargo build --release
cp target/release/codex-usage /usr/local/bin/

# or just run
cargo run --release
```

## Requirements

- Rust 1.75+
- macOS or Linux
- Logged in via `codex login` (for OAuth token)

## OpenClaw skill wrapper

If you use OpenClaw, the companion skill wrapper is here:
- https://github.com/davidfindlay/openclaw-codex-usage-skill

## License

GPL-3.0 (see `LICENSE`).

## Troubleshooting

- **Only API key found / no usage limits shown**
  - Run: `codex login`
  - OAuth session tokens are required for `/wham/usage` limits.
- **401/403 from usage endpoint**
  - Run: `codex logout && codex login`
- **No credentials found**
  - Check `~/.codex/auth.json` exists or set `CODEX_ACCESS_TOKEN`.

## Privacy & security

- This tool reads local auth credentials to call usage APIs.
- It does **not** print raw tokens.
- Avoid sharing screenshots/output publicly if account usage details are sensitive.
