//! Redaction shared by every surface that shows agent or terminal text
//! outside its terminal: Slack cards and the work board. One choke point, so
//! a fix lands everywhere.

const REDACTED: &str = "[redacted]";
const REDACTED_PATH: &str = "[path]";

/// Collapse control characters and whitespace, redact, and truncate to
/// `max_chars` (with an ellipsis). Not escaped for any markup.
#[must_use]
pub(crate) fn clean_text(value: &str, max_chars: usize) -> String {
    let collapsed = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let redacted = redact(&collapsed);
    if redacted.chars().count() > max_chars {
        let mut kept = redacted
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        kept.push('\u{2026}');
        kept
    } else {
        redacted
    }
}

/// Redact one line of a command an agent wants to run (or any prompt
/// line): every environment assignment's value (`FOO=bar` →
/// `FOO=[redacted]`), the value of a secret-looking flag (`--token x`,
/// `--password=x`), credentials in URLs, then everything [`redact`] covers
/// (tokens, key ids, paths, loopback URLs, `secret=value` pairs).
#[must_use]
pub fn redact_command(value: &str) -> String {
    /// Tools whose `-p` carries a password (`-phunter2`, `-p hunter2`).
    const PASSWORD_P_TOOLS: &[&str] = &["mysql", "mysqldump", "mysqladmin", "mariadb", "sshpass"];
    let mut words = Vec::new();
    let mut redact_next = false;
    let mut password_p = false;
    for word in value.split(' ') {
        if redact_next && !word.is_empty() {
            words.push(REDACTED.to_owned());
            redact_next = false;
            continue;
        }
        let bare = word.trim_start_matches(['"', '\'', '(', '`']);
        let program = bare.rsplit('/').next().unwrap_or(bare);
        if PASSWORD_P_TOOLS.contains(&program) {
            password_p = true;
        }
        if password_p && bare == "-p" {
            words.push(word.to_owned());
            redact_next = true;
            continue;
        }
        if password_p && bare.len() > 2 && bare.starts_with("-p") && !bare.starts_with("-p=") {
            words.push(format!("-p{REDACTED}"));
            continue;
        }
        // `curl -u user:pass`, `--user=user:pass`.
        if bare == "-u" || bare == "--user" {
            words.push(word.to_owned());
            redact_next = true;
            continue;
        }
        if let Some(value) = bare.strip_prefix("--user=")
            && !value.is_empty()
        {
            words.push(format!("--user={REDACTED}"));
            continue;
        }
        if let Some((name, _)) = bare.split_once('=')
            && environment_name(name)
        {
            words.push(format!("{name}={REDACTED}"));
            continue;
        }
        if let Some(flag) = bare.strip_prefix('-') {
            let flag = flag.trim_start_matches('-');
            match flag.split_once('=') {
                Some((name, _)) if secret_key(&name.to_ascii_lowercase()) => {
                    words.push(format!("--{name}={REDACTED}"));
                    continue;
                }
                None if !flag.is_empty() && secret_key(&flag.to_ascii_lowercase()) => {
                    words.push(word.to_owned());
                    redact_next = true;
                    continue;
                }
                _ => {}
            }
        }
        words.push(strip_url_credentials(word));
    }
    redact(&words.join(" "))
}

/// `FOO`, `AWS_SECRET_ACCESS_KEY`, `_X1`: a conventional environment name.
fn environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// `https://user:pass@host/x` → `https://[redacted]@host/x`.
fn strip_url_credentials(word: &str) -> String {
    let Some(scheme_end) = word.find("://") else {
        return word.to_owned();
    };
    let rest = &word[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{}://{REDACTED}@{}", &word[..scheme_end], &rest[at + 1..]),
        None => word.to_owned(),
    }
}

/// Replace anything that looks like a credential or a loopback URL.
///
/// Titles should never hold secrets; this is a defensive last line, so it
/// errs toward redacting: Slack and bearer tokens, AWS key ids, JWTs, PEM
/// markers, `key=value` pairs whose key names a secret, long opaque runs,
/// and URLs pointing at this machine.
#[must_use]
pub fn redact(value: &str) -> String {
    let mut words = Vec::new();
    let mut redact_next = false;
    // Inside an `Authorization:` value: every word up to the closing quote
    // (or the end of the line), so `Basic <b64>` loses its credential too.
    let mut in_header = false;
    let mut previous_secret = false;
    for word in value.split(' ') {
        if in_header && !word.is_empty() {
            match word.chars().last() {
                Some(quote @ ('"' | '\'')) => {
                    words.push(format!("{REDACTED}{quote}"));
                    in_header = false;
                }
                _ => words.push(REDACTED.to_owned()),
            }
            continue;
        }
        if redact_next && !word.is_empty() {
            words.push(REDACTED.to_owned());
            redact_next = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        let unquoted = lower.trim_start_matches(['"', '\'']);
        if unquoted == "authorization:" || unquoted == "proxy-authorization:" {
            in_header = true;
            words.push(word.to_owned());
            continue;
        }
        if lower == "bearer" {
            redact_next = true;
            words.push(word.to_owned());
            continue;
        }
        // `db_password = "hunter2"`: the value follows a lone `=` / `:`.
        if (word == "=" || word == ":") && previous_secret {
            redact_next = true;
            words.push(word.to_owned());
            continue;
        }
        previous_secret = !word.is_empty()
            && !word.contains(['=', ':'])
            && secret_key(unquoted.trim_end_matches(['"', '\'']));
        if sensitive_word(word, &lower) {
            words.push(REDACTED.to_owned());
            continue;
        }
        if path_word(word, &lower) {
            words.push(REDACTED_PATH.to_owned());
            continue;
        }
        match word.split_once(['=', ':']) {
            // `password: hunter2` — the value is the next word.
            Some((key, "")) if secret_key(&key.to_ascii_lowercase()) => {
                redact_next = true;
                words.push(word.to_owned());
            }
            Some((key, _)) if secret_key(&key.to_ascii_lowercase()) => {
                words.push(format!("{key}={REDACTED}"));
            }
            _ => words.push(word.to_owned()),
        }
    }
    words.join(" ")
}

fn sensitive_word(word: &str, lower: &str) -> bool {
    let trimmed = word.trim_matches(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '-' || character == '_')
    });
    let slack_token = [
        "xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-", "xoxe-", "xapp-",
    ]
    .iter()
    .any(|prefix| lower.contains(prefix));
    let aws_key_id = ["akia", "asia", "aida", "aroa"].iter().any(|prefix| {
        trimmed.len() == 20
            && trimmed.to_ascii_lowercase().starts_with(prefix)
            && trimmed
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    });
    let jwt = trimmed.starts_with("eyJ") && word.matches('.').count() >= 2;
    let pem = lower.contains("-----begin") || lower.contains("private key");
    // Any scheme (`http`, `https`, `ws`, `wss`, …) anywhere in the word, so
    // `(http://…)` and `See:http://…` are caught, plus scheme-less
    // `localhost:4317/…` and `127.0.0.1:4317/…`.
    let loopback_host = |rest: &str| {
        ["127.", "localhost", "[::1]", "::1", "0.0.0.0"]
            .iter()
            .any(|host| rest.starts_with(host))
    };
    let bare = lower.trim_start_matches(|character: char| !character.is_ascii_alphanumeric());
    let loopback_url = lower
        .match_indices("://")
        .any(|(index, _)| loopback_host(&lower[index + 3..]))
        || ["localhost:", "localhost/", "127.0.0.1:", "127.0.0.1/"]
            .iter()
            .any(|prefix| bare.starts_with(prefix));
    let opaque = trimmed.len() >= 32
        && trimmed.bytes().any(|byte| byte.is_ascii_digit())
        && trimmed.bytes().any(|byte| byte.is_ascii_alphabetic())
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+/=_-".contains(&byte));
    slack_token || aws_key_id || jwt || pem || loopback_url || opaque
}

/// A local file path: `/a/b`, `~/a`, `./a/b`, `../a`, or anything naming a
/// home directory. Paths are local detail and never belong in a DM.
fn path_word(word: &str, lower: &str) -> bool {
    if lower.contains("/users/") || lower.contains("/home/") || lower.contains("~/") {
        return true;
    }
    let stripped = word.trim_start_matches(|character: char| "([{\"'`<".contains(character));
    let rooted = ["/", "./", "../"]
        .iter()
        .any(|prefix| stripped.starts_with(prefix));
    rooted
        && stripped
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
            .count()
            >= 2
}

fn secret_key(key: &str) -> bool {
    [
        "token",
        "secret",
        "password",
        "passwd",
        "cookie",
        "session",
        "apikey",
        "api_key",
        "credential",
        "auth",
        "api-key",
        "_key",
        "-key",
        "private",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}
