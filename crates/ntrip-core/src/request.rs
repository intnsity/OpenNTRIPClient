//! Request construction: exact wire bytes plus the per-line log copies.
//!
//! Two invariants live here, both security-shaped:
//! - The LOG copies never carry credentials. Event and connection logs are
//!   routinely pasted into support tickets, and `Basic` is base64, not
//!   encryption - so the Authorization value is masked in the returned lines
//!   while the wire bytes stay exact.
//! - No caller-provided string can forge request lines. Mountpoints are
//!   free text (spaces and worse do occur in the wild, and settings.toml is
//!   hand-editable), so the mountpoint is percent-encoded into the request
//!   line and every header value is stripped of control characters before
//!   it touches the wire. Credentials are immune to header injection (they
//!   ride inside base64) but CR/LF is stripped from them anyway: no byte a
//!   caster could never have seen in a login form belongs in the encoding.

use crate::{NtripVersion, SessionConfig, base64};

/// Build the request for an Ntrip-transport session. Returns the request
/// lines for the connection log (Authorization value masked) and the wire
/// bytes (lines joined with CRLF, terminated by an empty line). Header order
/// is fixed and pinned by tests: interop with quirky casters was validated
/// against this exact ordering.
pub(crate) fn build(cfg: &SessionConfig) -> (Vec<String>, Vec<u8>) {
    let mountpoint = encode_mountpoint(&cfg.mountpoint);
    let host = clean_header(&cfg.host);
    let user_agent = clean_header(&cfg.user_agent);
    let mut lines: Vec<String> = Vec::with_capacity(7);
    match cfg.version {
        NtripVersion::V1 => {
            lines.push(format!("GET /{mountpoint} HTTP/1.0"));
            lines.push(format!("User-Agent: {user_agent}"));
            lines.push("Accept: */*".to_string());
            lines.push("Connection: close".to_string());
            if !cfg.username.is_empty() {
                lines.push(auth_line(cfg));
            }
        }
        NtripVersion::V2 => {
            lines.push(format!("GET /{mountpoint} HTTP/1.1"));
            lines.push(format!("Host: {}:{}", host, cfg.port));
            lines.push("Ntrip-Version: Ntrip/2.0".to_string());
            lines.push(format!("User-Agent: {user_agent}"));
            lines.push("Accept: */*".to_string());
            if !cfg.username.is_empty() {
                lines.push(auth_line(cfg));
            }
            lines.push("Connection: close".to_string());
        }
    }
    let mut wire = String::new();
    for line in &lines {
        wire.push_str(line);
        wire.push_str("\r\n");
    }
    wire.push_str("\r\n");
    let log_lines = lines.iter().map(|l| mask_auth(l)).collect();
    (log_lines, wire.into_bytes())
}

fn auth_line(cfg: &SessionConfig) -> String {
    let credentials = format!(
        "{}:{}",
        strip_crlf(&cfg.username),
        strip_crlf(&cfg.password)
    );
    format!(
        "Authorization: Basic {}",
        base64::encode(credentials.as_bytes())
    )
}

/// Log copy of a wire line: the reversible Basic credential is masked, and
/// nothing else is touched - full protocol verbosity is the product.
fn mask_auth(line: &str) -> String {
    if line.starts_with("Authorization:") {
        "Authorization: Basic ****".to_string()
    } else {
        line.to_string()
    }
}

/// Header values must be a single line: strip every ASCII control character
/// (CR/LF would split the request into forged lines; the rest are equally
/// meaningless in a header).
fn clean_header(value: &str) -> String {
    value.chars().filter(|c| !c.is_ascii_control()).collect()
}

fn strip_crlf(value: &str) -> String {
    value.chars().filter(|c| *c != '\r' && *c != '\n').collect()
}

/// Percent-encode the bytes that would break the request line: controls and
/// space (a three-token-plus GET line is malformed), DEL, and non-ASCII
/// (encoded per the URI rules rather than sent raw). '%' itself passes
/// through so users who already type pre-encoded mountpoints stay untouched.
fn encode_mountpoint(mount: &str) -> String {
    let mut out = String::with_capacity(mount.len());
    for b in mount.bytes() {
        if b <= 0x20 || b >= 0x7f {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::build;
    use crate::{GgaPolicy, NtripVersion, SessionConfig, Transport};

    fn cfg(version: NtripVersion, mountpoint: &str, username: &str) -> SessionConfig {
        SessionConfig {
            host: "caster.example.com".to_string(),
            port: 2101,
            mountpoint: mountpoint.to_string(),
            username: username.to_string(),
            password: "pass".to_string(),
            version,
            transport: Transport::Ntrip,
            user_agent: "NTRIP OpenNtripClient/0.1.0".to_string(),
            gga: GgaPolicy::Off,
        }
    }

    #[test]
    fn v1_with_auth_golden() {
        let (lines, wire) = build(&cfg(NtripVersion::V1, "MOUNT", "user"));
        assert_eq!(
            lines,
            vec![
                "GET /MOUNT HTTP/1.0",
                "User-Agent: NTRIP OpenNtripClient/0.1.0",
                "Accept: */*",
                "Connection: close",
                // Log copy: masked. The wire carries the real value.
                "Authorization: Basic ****",
            ]
        );
        assert_eq!(
            wire,
            b"GET /MOUNT HTTP/1.0\r\n\
              User-Agent: NTRIP OpenNtripClient/0.1.0\r\n\
              Accept: */*\r\n\
              Connection: close\r\n\
              Authorization: Basic dXNlcjpwYXNz\r\n\
              \r\n"
        );
    }

    #[test]
    fn v2_with_auth_golden() {
        let (lines, wire) = build(&cfg(NtripVersion::V2, "MOUNT", "user"));
        assert_eq!(
            lines,
            vec![
                "GET /MOUNT HTTP/1.1",
                "Host: caster.example.com:2101",
                "Ntrip-Version: Ntrip/2.0",
                "User-Agent: NTRIP OpenNtripClient/0.1.0",
                "Accept: */*",
                "Authorization: Basic ****",
                "Connection: close",
            ]
        );
        // b64("user:pass") goes on the wire, exactly.
        assert!(wire_text(&wire).contains("Authorization: Basic dXNlcjpwYXNz\r\n"));
        assert!(wire.ends_with(b"Connection: close\r\n\r\n"));
    }

    #[test]
    fn no_auth_header_when_username_empty() {
        let (lines, _) = build(&cfg(NtripVersion::V1, "MOUNT", ""));
        assert!(lines.iter().all(|l| !l.starts_with("Authorization")));
        let (lines, _) = build(&cfg(NtripVersion::V2, "MOUNT", ""));
        assert!(lines.iter().all(|l| !l.starts_with("Authorization")));
    }

    #[test]
    fn empty_mountpoint_is_sourcetable_request() {
        let (lines, _) = build(&cfg(NtripVersion::V1, "", "user"));
        assert_eq!(lines[0], "GET / HTTP/1.0");
        let (lines, _) = build(&cfg(NtripVersion::V2, "", "user"));
        assert_eq!(lines[0], "GET / HTTP/1.1");
    }

    #[test]
    fn wire_is_lines_joined_with_crlf_and_blank_terminator() {
        let (lines, wire) = build(&cfg(NtripVersion::V2, "X", "u"));
        // Lines and wire agree everywhere except the masked credential.
        let expected = lines.join("\r\n") + "\r\n\r\n";
        assert_eq!(
            wire_text(&wire).replace("Basic dTpwYXNz", "Basic ****"),
            expected
        );
    }

    /// The leak regression: no log line may contain the reversible
    /// credential, in any version, ever. b64("user:pass") = dXNlcjpwYXNz.
    #[test]
    fn log_lines_never_carry_credentials() {
        for version in [NtripVersion::V1, NtripVersion::V2] {
            let (lines, wire) = build(&cfg(version, "MOUNT", "user"));
            assert!(
                lines.iter().all(|l| !l.contains("dXNlcjpwYXNz")),
                "credential leaked into log lines: {lines:?}"
            );
            assert!(
                lines.iter().any(|l| l == "Authorization: Basic ****"),
                "masked auth line must still be visible in the log"
            );
            assert!(
                wire_text(&wire).contains("dXNlcjpwYXNz"),
                "the wire must keep the real credential"
            );
        }
    }

    #[test]
    fn mountpoint_spaces_and_controls_are_percent_encoded() {
        let (lines, wire) = build(&cfg(NtripVersion::V1, "MY MOUNT", "user"));
        assert_eq!(lines[0], "GET /MY%20MOUNT HTTP/1.0");
        assert!(wire_text(&wire).starts_with("GET /MY%20MOUNT HTTP/1.0\r\n"));

        // CR/LF cannot forge lines; DEL and non-ASCII are encoded too.
        let (lines, _) = build(&cfg(NtripVersion::V1, "M\r\nInjected: yes", "user"));
        assert_eq!(lines[0], "GET /M%0D%0AInjected:%20yes HTTP/1.0");
        let (lines, _) = build(&cfg(NtripVersion::V2, "M\u{7f}\u{e9}", ""));
        assert_eq!(lines[0], "GET /M%7F%C3%A9 HTTP/1.1");

        // Pre-encoded input passes through untouched.
        let (lines, _) = build(&cfg(NtripVersion::V1, "MY%20MOUNT", ""));
        assert_eq!(lines[0], "GET /MY%20MOUNT HTTP/1.0");
    }

    #[test]
    fn header_values_cannot_inject_lines() {
        let mut c = cfg(NtripVersion::V2, "M", "user");
        c.host = "evil.example\r\nX-Forged: 1".to_string();
        c.user_agent = "agent\r\nX-Forged: 2".to_string();
        c.password = "p\r\nass".to_string();
        let (lines, wire) = build(&c);
        let text = wire_text(&wire);
        // The hostile text survives INSIDE the header value (harmless, and
        // visible in the log for diagnosis); it must never start a line.
        assert!(!text.contains("\r\nX-Forged"), "{text}");
        assert!(
            lines
                .iter()
                .any(|l| l == "Host: evil.exampleX-Forged: 1:2101"),
            "{lines:?}"
        );
        // CR/LF is stripped from credentials before encoding: b64("user:pass").
        assert!(text.contains("Authorization: Basic dXNlcjpwYXNz"), "{text}");
        // Exactly one blank line, the terminator: no forged early end.
        assert_eq!(text.matches("\r\n\r\n").count(), 1);
    }

    fn wire_text(wire: &[u8]) -> &str {
        std::str::from_utf8(wire).unwrap()
    }
}
